import { screen, waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { ENVS, KUBE_ENV, kubeRoutes, loggedIn, renderApp } from "@/test/app";
import { mockFetch } from "@/test/fetch";

const viewer = <T extends object>(e: T) => ({ ...e, access: { role: "viewer", namespaces: null } });

describe("a viewer", () => {
  it("sees Docker pages without write controls", async () => {
    mockFetch({ ...loggedIn, "GET /api/v1/envs": { json: ENVS.map(viewer) } });
    renderApp("/envs/local/containers");
    await screen.findByRole("heading", { name: "Containers" });
    await screen.findByText("No containers in this environment.");
    expect(screen.queryByRole("link", { name: /Create container/ })).toBeNull();
    expect(screen.queryByRole("button", { name: /Prune/ })).toBeNull();
  });

  it("sees Kubernetes lists without Create or Apply YAML", async () => {
    mockFetch({ ...loggedIn, ...kubeRoutes, "GET /api/v1/envs": { json: [...ENVS, viewer(KUBE_ENV)] } });
    renderApp("/envs/k/k8s/deployments");
    await screen.findByRole("table", { name: "Deployments" });
    await waitFor(() => expect(screen.queryByRole("button", { name: "Create" })).toBeNull());
    expect(screen.queryByRole("button", { name: /Apply YAML/ })).toBeNull();
  });

  it("is told about a stale namespace and can pick another", async () => {
    localStorage.setItem("fleetops.ns.k", "team-b");
    mockFetch({
      ...loggedIn,
      ...kubeRoutes,
      "GET /api/v1/envs": { json: [...ENVS, { ...KUBE_ENV, access: { role: "viewer", namespaces: ["team-a"] } }] },
      "GET /api/v1/envs/k/k8s/deployments": { status: 403, json: { code: "forbidden", message: "namespace 'team-b' is not in your access" } },
    });
    renderApp("/envs/k/k8s/deployments");
    expect(await screen.findByText(/No permission to list Deployments in namespace team-b/)).toBeInTheDocument();
    expect(screen.getByRole("combobox", { name: "Namespace" })).toBeInTheDocument();
    localStorage.clear();
  });
});
