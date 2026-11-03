import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { ImageBadge, ImageDetail } from "@/api/gen/types.gen";
import { mockFetch } from "@/test/fetch";
import { loggedIn, renderApp } from "@/test/app";
import { image } from "@/test/docker";

const A = image({ id: "sha256:aaa111222333444", repo_tags: ["nginx:1.25"], in_use: true });
const B = image({ id: "sha256:bbb111222333444", repo_tags: ["redis:7"] });
const C = image({ id: "sha256:ccc111222333444", repo_tags: ["bad:1"] });

const badges: ImageBadge[] = [
  { image_id: A.id, severity: "critical", fixable: 4, state: "scanned" },
  { image_id: B.id, severity: null, fixable: 0, state: "never_scanned" },
  { image_id: C.id, severity: null, fixable: 0, state: "failed" },
];

const routes = (extra: Record<string, unknown> = {}) => ({
  ...loggedIn,
  "GET /api/v1/edition": { json: { edition: "enterprise", features: ["vuln_scan"] } },
  "GET /api/v1/envs/local/images": { json: [A, B, C] },
  "GET /api/v1/ee/security/envs/local/badges": { json: badges },
  ...extra,
});

const rows = () => within(screen.getByRole("table", { name: "Images" })).getAllByRole("row").slice(1);

describe("image list badges", () => {
  it("asks once for the whole list", async () => {
    const calls = mockFetch(routes());
    renderApp("/envs/local/images");
    expect(await screen.findByText("● Critical 4")).toBeInTheDocument();
    expect(calls.filter((c) => c.path.startsWith("/api/v1/ee/security/envs/local/badges"))).toHaveLength(1);
    expect(screen.getByRole("columnheader", { name: /Security/ })).toBeInTheDocument();
  });

  it("marks failed and unscanned images", async () => {
    mockFetch(routes());
    renderApp("/envs/local/images");
    await waitFor(() => expect(rows()).toHaveLength(3));
    expect(await screen.findByText("Scan failed")).toBeInTheDocument();
    expect(screen.getByText("Not scanned")).toBeInTheDocument();
  });
});

describe("image page tab", () => {
  const IMG = { Id: A.id, RepoTags: ["nginx:1.25"], Created: "2026-09-01T00:00:00Z", Size: 1, Os: "linux", Architecture: "amd64", Config: {} };
  const detail: ImageDetail = {
    image: "nginx:1.25",
    digests: [
      {
        key: "sha256:abc",
        state: "scanned",
        last_scan_at: Math.floor(Date.now() / 1000) - 60,
        trivy_image: "aquasec/trivy:0.76.1",
        error: null,
        counts: { critical: 1, high: 0, medium: 0, low: 0, unknown: 0, critical_fixable: 1, high_fixable: 0 },
        findings: [
          { vuln_id: "CVE-2024-5535", severity: "critical", pkg_name: "libssl3", installed_version: "3.0.11", fixed_version: "3.0.14", title: null, url: null, first_seen_at: 1 },
        ],
        used_by: [{ env: "local", node: "", namespace: "", workload: "", container: "web", running: true }],
      },
    ],
  };

  it("shows findings and Scan now", async () => {
    const calls = mockFetch(
      routes({
        "GET /api/v1/envs/local/images/sha256%3Aaaa111222333444": { json: IMG },
        "GET /api/v1/envs/local/images/sha256%3Aaaa111222333444/history": { json: [] },
        "GET /api/v1/ee/security/envs/local/images/sha256%3Aaaa111222333444": { json: detail },
        "POST /api/v1/ee/security/scan": { json: { queued: 1 } },
      }),
    );
    renderApp("/envs/local/images/sha256%3Aaaa111222333444");
    await userEvent.click(await screen.findByRole("tab", { name: "Vulnerabilities" }));
    expect(await screen.findByText("CVE-2024-5535")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Scan now" }));
    await waitFor(() => expect(calls.find((c) => c.method === "POST")?.body).toEqual({ env: "local", images: ["sha256:abc"] }));
  });
});
