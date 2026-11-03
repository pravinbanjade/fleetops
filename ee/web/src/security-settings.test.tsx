import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { SecuritySettings } from "@/api/gen/types.gen";
import { mockFetch } from "@/test/fetch";
import { ADMIN, loggedIn, renderApp } from "@/test/app";
import { EnvSecuritySettings } from "./EnvSecuritySettings";

const DEFAULT_IMAGE = "aquasec/trivy:0.76.1@sha256:790fd66d";

const settings = (over: Partial<SecuritySettings> = {}): SecuritySettings => ({
  global: { enabled: false, mirror: null, default_trivy_image: DEFAULT_IMAGE },
  envs: [
    { env: "local", kind: "docker-local", enabled: null, effective_enabled: false, mirror: null, socket_path: null, status: { state: "off" } },
  ],
  ...over,
});

const page = (s: SecuritySettings) => ({
  ...loggedIn,
  "GET /api/v1/edition": { json: { edition: "enterprise", features: ["vuln_scan"] } },
  "GET /api/v1/ee/security/settings": { json: s },
});

describe("security scanning settings", () => {
  it("turning on asks first and names the image", async () => {
    const calls = mockFetch({
      ...page(settings()),
      "PUT /api/v1/ee/security/settings": { json: settings({ global: { enabled: true, mirror: null, default_trivy_image: DEFAULT_IMAGE } }) },
    });
    renderApp("/admin/security");
    await userEvent.click(await screen.findByRole("button", { name: "Turn on" }));
    const dialog = await screen.findByRole("dialog", { name: "Turn on security scanning?" });
    expect(dialog).toHaveTextContent(DEFAULT_IMAGE);
    expect(dialog).toHaveTextContent("vulnerability database");
    expect(calls.some((c) => c.method === "PUT")).toBe(false);
    await userEvent.click(within(dialog).getByRole("button", { name: "Turn on" }));
    await waitFor(() => expect(calls.find((c) => c.method === "PUT")?.body).toMatchObject({ enabled: true }));
  });

  it("keeps the password write-only", async () => {
    const mirror = { trivy_image: "mirror.local/trivy:1", db_repository: null, java_db_repository: null, username: "bot", password_set: true };
    const calls = mockFetch({
      ...page(settings({ global: { enabled: true, mirror, default_trivy_image: DEFAULT_IMAGE } })),
      "PUT /api/v1/ee/security/settings": { json: settings() },
    });
    renderApp("/admin/security");
    const pw = await screen.findByLabelText("Password");
    expect(pw).toHaveValue("");
    expect(screen.getByText("Saved")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Save mirror" }));
    await waitFor(() => expect(calls.find((c) => c.method === "PUT")).toBeTruthy());
    const body = calls.find((c) => c.method === "PUT")?.body as { mirror: Record<string, unknown> };
    expect(body.mirror.password).toBeUndefined();
    expect(body.mirror.username).toBe("bot");
  });

  it("shows why a scanner is unavailable", async () => {
    mockFetch(
      page(
        settings({
          envs: [
            {
              env: "local",
              kind: "docker-local",
              enabled: true,
              effective_enabled: true,
              mirror: null,
              socket_path: null,
              status: { state: "unavailable", reason: "pulling aquasec/trivy: pull failed" },
            },
          ],
        }),
      ),
    );
    renderApp("/admin/security");
    expect(await screen.findByText("Unavailable: pulling aquasec/trivy: pull failed")).toBeInTheDocument();
  });

  it("never asks a non-admin for the settings", async () => {
    const calls = mockFetch({
      ...page(settings()),
      "GET /api/v1/auth/me": { json: { ...ADMIN, user: { ...ADMIN.user, role: "user" } } },
    });
    renderApp("/admin/security");
    await waitFor(() => expect(calls.some((c) => c.path.startsWith("/api/v1/envs"))).toBe(true));
    expect(calls.some((c) => c.path.startsWith("/api/v1/ee/security/settings"))).toBe(false);
  });
});

describe("environment scanning panel", () => {
  const renderPanel = (kind: string) => {
    mockFetch(page(settings({ envs: [{ ...settings().envs[0], kind: kind as "docker-local" }] })));
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={qc}>
        <EnvSecuritySettings env="local" kind={kind} />
      </QueryClientProvider>,
    );
  };

  it("offers a socket path on Docker hosts", async () => {
    renderPanel("docker-local");
    expect(await screen.findByLabelText("Engine socket")).toBeInTheDocument();
  });

  it("has no socket path on clusters", async () => {
    renderPanel("kube-config");
    expect(await screen.findByLabelText("Security scanning")).toBeInTheDocument();
    expect(screen.queryByLabelText("Engine socket")).toBeNull();
  });
});
