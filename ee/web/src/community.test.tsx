import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen as rtl, waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { mockFetch } from "@/test/fetch";
import { ADMIN, loggedIn, renderApp } from "@/test/app";
import { UpgradeCard } from "./UpgradeCard";

const VERSION = import.meta.env.FLEETOPS_VERSION;
const PRICING = "https://fleetops.pravinb.com.np/pricing";

/** A Community install with no leftover Enterprise data. */
const community = {
  ...loggedIn,
  "GET /api/v1/ee/teams": { json: [] },
  "GET /api/v1/ee/roles": { json: [] },
  "GET /api/v1/ee/sso/connections": { json: [] },
  "GET /api/v1/ee/backup/schedule": { json: { enabled: false, hour_utc: 2, retention: { kind: "count", value: 7 }, passphrase_set: false } },
  "GET /api/v1/ee/backup/targets": { json: [] },
  "GET /api/v1/ee/backup/status": {
    json: { licensed: false, enabled: false, paused: false, last_run: null, last_failed: false, next_run_at: null },
  },
  "GET /api/v1/ee/license": { json: { status: "none", source: null, reason: null, id: null, customer: null, expires_at: null, max_users: null, features: [], users_in_use: 1, max_nodes: null, nodes_in_use: 1 } },
  "GET /api/v1/audit": { json: [] },
};

const adminNav = () => rtl.findByRole("navigation", { name: "Administration" });

describe("Community plan", () => {
  it("admin on Community sees label, card and pricing link", async () => {
    mockFetch(community);
    const { screen } = renderApp("/admin/audit");
    expect(await screen.findByText(`Community ${VERSION}`)).toBeInTheDocument();
    const upgrade = await screen.findByRole("link", { name: "Upgrade" });
    expect(upgrade).toHaveAttribute("href", PRICING);
    expect(upgrade).toHaveAttribute("target", "_blank");
  });

  it("hides Teams, Roles and Single sign-on without leftovers", async () => {
    mockFetch(community);
    renderApp("/admin/audit");
    const nav = await adminNav();
    await waitFor(() => expect(nav).toHaveTextContent("License"));
    expect(nav).not.toHaveTextContent("Teams");
    expect(nav).not.toHaveTextContent("Roles");
    expect(nav).not.toHaveTextContent("Single sign-on");
  });

  it("shows Teams when teams are left over", async () => {
    mockFetch({ ...community, "GET /api/v1/ee/teams": { json: [{ id: 7, name: "ops", description: "", members: 0, environments: 0 }] } });
    renderApp("/admin/audit");
    const nav = await adminNav();
    await waitFor(() => expect(nav).toHaveTextContent("Teams"));
    expect(nav).not.toHaveTextContent("Roles");
  });

  it("hides scheduled backups without leftovers", async () => {
    const calls = mockFetch(community);
    const { screen } = renderApp("/admin/backup");
    await screen.findByRole("button", { name: "Download backup" });
    await waitFor(() => expect(calls.some((c) => c.path === "/api/v1/ee/backup/targets")).toBe(true));
    await new Promise((r) => setTimeout(r, 50));
    expect(screen.queryByText("Scheduled backups")).toBeNull();
  });

  it("shows scheduled backups when a target is left over", async () => {
    mockFetch({
      ...community,
      "GET /api/v1/ee/backup/targets": { json: [{ kind: "local", settings: { kind: "local", dir: "/backups" }, secret_set: false }] },
    });
    const { screen } = renderApp("/admin/backup");
    expect(await screen.findByText("Scheduled backups")).toBeInTheDocument();
  });

  it("failed leftover check hides the item", async () => {
    mockFetch({ ...community, "GET /api/v1/ee/teams": { status: 500, json: { code: "internal", message: "boom" } } });
    renderApp("/admin/audit");
    const nav = await adminNav();
    await waitFor(() => expect(nav).toHaveTextContent("License"));
    expect(nav).not.toHaveTextContent("Teams");
    expect(rtl.queryByText("boom")).toBeNull();
  });

  it("non-admin sees label only and makes no ee calls", async () => {
    const calls = mockFetch({
      ...community,
      "GET /api/v1/auth/me": { json: { ...ADMIN, user: { ...ADMIN.user, role: "user" } } },
    });
    const { screen } = renderApp("/");
    expect(await screen.findByText(`Community ${VERSION}`)).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Upgrade" })).toBeNull();
    expect(calls.some((c) => c.path.startsWith("/api/v1/ee/"))).toBe(false);
  });

  it("Enterprise plan shows Enterprise label and no card", async () => {
    mockFetch({
      ...community,
      "GET /api/v1/edition": { json: { edition: "enterprise", features: ["teams", "rbac", "sso", "backup", "audit_export"] } },
    });
    const { screen } = renderApp("/admin/audit");
    expect(await screen.findByText(`Enterprise ${VERSION}`)).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Upgrade" })).toBeNull();
  });

  it("label shows version alone until the plan loads", async () => {
    mockFetch({ ...community, "GET /api/v1/edition": () => new Promise(() => {}) });
    const { screen } = renderApp("/admin/audit");
    expect(await screen.findByText(VERSION)).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Upgrade" })).toBeNull();
  });

  it("collapsed sidebar shows an upgrade icon link", async () => {
    mockFetch(community);
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={qc}>
        <UpgradeCard collapsed />
      </QueryClientProvider>,
    );
    const link = await rtl.findByRole("link", { name: "Upgrade to Enterprise" });
    expect(link).toHaveAttribute("href", PRICING);
  });
});
