import { waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { mockFetch } from "@/test/fetch";
import { loggedIn, renderApp } from "@/test/app";
import { bannerFor, GRACE_SECS, type LicenseInfo } from "./license";

const NOW = 1_800_000_000;
const info = (over: Partial<LicenseInfo> = {}): LicenseInfo => ({
  status: "valid",
  source: "uploaded",
  reason: null,
  id: "lic-1",
  customer: "Acme",
  expires_at: NOW + 100 * 86_400,
  max_users: 5,
  features: ["audit_export"],
  users_in_use: 1,
  ...over,
});

describe("bannerFor", () => {
  it("says nothing for a license far from expiry or no license", () => {
    expect(bannerFor(info(), NOW)).toBeNull();
    expect(bannerFor(info({ status: "none" }), NOW)).toBeNull();
  });
  it("warns in the last 14 days, in grace, expired and invalid", () => {
    expect(bannerFor(info({ expires_at: NOW + 86_400 }), NOW)).toMatch(/expires on/);
    expect(bannerFor(info({ status: "grace", expires_at: NOW - 86_400 }), NOW)).toMatch(/turn off on/);
    expect(bannerFor(info({ status: "expired" }), NOW)).toMatch(/has expired/);
    expect(bannerFor(info({ status: "invalid", reason: "signature does not verify" }), NOW)).toMatch(
      /signature does not verify/,
    );
    expect(GRACE_SECS).toBe(14 * 86_400);
  });
});

describe("enterprise UI", () => {
  it("installs a license from the License page", async () => {
    let current = info({ status: "none", customer: null, id: null, expires_at: null, max_users: null, features: [] });
    const calls = mockFetch({
      ...loggedIn,
      "GET /api/v1/edition": () => ({ json: { edition: "enterprise", features: current.features } }),
      "GET /api/v1/ee/license": () => ({ json: current }),
      "PUT /api/v1/ee/license": () => {
        current = info();
        return { json: current };
      },
    });
    const { screen } = renderApp("/admin/license");
    await screen.findByLabelText("License key");
    expect(screen.getByRole("link", { name: "License" })).toBeInTheDocument();
    await userEvent.type(screen.getByLabelText("License key"), "abc.def");
    await userEvent.click(screen.getByRole("button", { name: "Install license" }));
    await screen.findByText("Acme");
    const put = calls.find((c) => c.method === "PUT");
    expect(put?.body).toBe("abc.def");
    expect(put?.headers.get("content-type")).toBe("text/plain");
  });

  it("lists the paid plans with yearly and monthly prices", async () => {
    mockFetch({
      ...loggedIn,
      "GET /api/v1/edition": { json: { edition: "enterprise", features: [] } },
      "GET /api/v1/ee/license": {
        json: info({ status: "none", source: null, customer: null, id: null, expires_at: null, features: [] }),
      },
    });
    const { screen } = renderApp("/admin/license");
    const scale = await screen.findByRole("article", { name: "Scale plan" });
    expect(scale).toHaveTextContent("$199");
    expect(scale).toHaveTextContent("Recommended");
    expect(screen.getByRole("article", { name: "Enterprise plan" })).toHaveTextContent("Custom");
    await userEvent.click(screen.getByRole("button", { name: "Monthly" }));
    expect(scale).toHaveTextContent("$19");
    expect(screen.getByText(/No license installed/)).toBeInTheDocument();
  });

  it("offers Remove only for an uploaded license", async () => {
    mockFetch({
      ...loggedIn,
      "GET /api/v1/edition": { json: { edition: "enterprise", features: ["audit_export"] } },
      "GET /api/v1/ee/license": { json: info({ source: "file" }) },
    });
    const { screen } = renderApp("/admin/license");
    await screen.findByText("Acme");
    expect(screen.queryByRole("button", { name: "Remove license" })).not.toBeInTheDocument();
    expect(screen.getByText(/--license-file/)).toBeInTheDocument();
  });

  it("shows the expiry banner and hides Export when the license lapsed", async () => {
    mockFetch({
      ...loggedIn,
      "GET /api/v1/audit": { json: [] },
      "GET /api/v1/edition": { json: { edition: "enterprise", features: [] } },
      "GET /api/v1/ee/license": { json: info({ status: "expired", expires_at: NOW - 30 * 86_400 }) },
    });
    const { screen } = renderApp("/admin/audit");
    await screen.findByText(/has expired/);
    await screen.findByRole("table", { name: "Audit log" });
    expect(screen.queryByRole("button", { name: "Export" })).not.toBeInTheDocument();
  });

  it("offers CSV and NDJSON downloads when audit export is licensed", async () => {
    mockFetch({
      ...loggedIn,
      "GET /api/v1/audit": { json: [] },
      "GET /api/v1/edition": { json: { edition: "enterprise", features: ["audit_export"] } },
      "GET /api/v1/ee/license": { json: info() },
    });
    const { screen } = renderApp("/admin/audit");
    await userEvent.click(await screen.findByRole("button", { name: "Export" }));
    await waitFor(() =>
      expect(screen.getByRole("menuitem", { name: "CSV" })).toHaveAttribute("href", "/api/v1/ee/audit/export?format=csv"),
    );
    expect(screen.getByRole("menuitem", { name: "NDJSON" })).toHaveAttribute(
      "href",
      "/api/v1/ee/audit/export?format=ndjson",
    );
  });
});
