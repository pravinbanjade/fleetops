import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { kubeRoutes, loggedIn, renderApp } from "@/test/app";
import { mockFetch } from "@/test/fetch";

describe("Kubernetes environments", () => {
  it("shows the Kubernetes pages in the sidebar, not Docker's", async () => {
    mockFetch({ ...loggedIn, ...kubeRoutes });
    renderApp("/envs/k");
    const nav = await screen.findByRole("navigation", { name: "Environment" });
    for (const label of ["Overview", "Workloads", "Pods", "Services", "Ingresses", "ConfigMaps", "Secrets", "Namespaces", "Nodes"]) {
      expect(within(nav).getByRole("link", { name: new RegExp(label) })).toBeInTheDocument();
    }
    expect(within(nav).queryByRole("link", { name: /Containers/ })).not.toBeInTheDocument();
    expect(await screen.findByText("v1.32.0")).toBeInTheDocument();
    expect(screen.getByText("3 of 3 nodes ready")).toBeInTheDocument();
  });

  it("lists a kind with its own columns", async () => {
    mockFetch({ ...loggedIn, ...kubeRoutes });
    renderApp("/envs/k/k8s/deployments");
    const table = await screen.findByRole("table", { name: "Deployments" });
    for (const h of ["Name", "Namespace", "Ready", "Up-to-date", "Available", "Age"]) {
      expect(within(table).getByRole("columnheader", { name: new RegExp(`^${h}(?!space)`) })).toBeInTheDocument();
    }
    expect(await within(table).findByRole("link", { name: "web" })).toHaveAttribute("href", "/envs/k/k8s/deployments/apps/web");
    expect(within(table).getAllByText("2/3").length).toBeGreaterThan(0);
    // Workload kinds share tabs.
    expect(screen.getByRole("link", { name: "StatefulSets" })).toHaveAttribute("href", "/envs/k/k8s/statefulsets");
  });

  it("asks for one namespace once it is picked, and remembers it", async () => {
    const calls = mockFetch({ ...loggedIn, ...kubeRoutes, "GET /api/v1/envs/k/k8s/pods": { json: { rows: [], truncated: false } } });
    renderApp("/envs/k/k8s/pods");
    const picker = await screen.findByRole("combobox", { name: "Namespace" });
    await within(picker).findByRole("option", { name: "apps" });
    await userEvent.selectOptions(picker, "apps");
    await screen.findByRole("table", { name: "Pods" });
    expect(calls.some((c) => c.path === "/api/v1/envs/k/k8s/pods?namespace=apps")).toBe(true);
    expect(localStorage.getItem("fleetops.ns.k")).toBe("apps");
  });

  it("says when the cluster forbids a list", async () => {
    mockFetch({
      ...loggedIn,
      ...kubeRoutes,
      "GET /api/v1/envs/k/k8s/secrets": { status: 403, json: { code: "forbidden", message: "secrets is forbidden", request_id: null } },
    });
    renderApp("/envs/k/k8s/secrets");
    expect(await screen.findByText("No permission to list Secrets in all namespaces")).toBeInTheDocument();
  });

  it("says when a list is truncated", async () => {
    mockFetch({ ...loggedIn, ...kubeRoutes, "GET /api/v1/envs/k/k8s/pods": { json: { rows: [], truncated: true } } });
    renderApp("/envs/k/k8s/pods");
    expect(await screen.findByText(/Showing the first 5,000/)).toBeInTheDocument();
  });

  it("docker_routes_under_a_kube_env_redirect_to_its_overview", async () => {
    const calls = mockFetch({ ...loggedIn, ...kubeRoutes });
    const { history } = renderApp("/envs/k/containers");
    expect(await screen.findByText("v1.32.0")).toBeInTheDocument();
    expect(history.location.pathname).toBe("/envs/k");
    expect(calls.some((c) => c.path === "/api/v1/envs/k/containers")).toBe(false);
  });

  it("does not subscribe to Docker events for a kube environment", async () => {
    mockFetch({ ...loggedIn, ...kubeRoutes });
    renderApp("/envs/k");
    await screen.findByText("v1.32.0");
    const { FakeWebSocket } = await import("@/test/fake-ws");
    const subs = FakeWebSocket.latest()?.frames().filter((m) => m.topic === "events" && m.env === "k") ?? [];
    expect(subs).toEqual([]);
  });
});

describe("a cluster whose credentials cannot list nodes", () => {
  it("is up and says the node count is not visible", async () => {
    mockFetch({
      ...loggedIn,
      ...kubeRoutes,
      "GET /api/v1/envs/k/status": {
        json: { state: "up", version: { kind: "kube", git_version: "v1.32.0", platform: "linux/amd64", nodes: null, nodes_ready: null } },
      },
    });
    renderApp("/envs/k");
    expect(await screen.findByText("Not visible to these credentials")).toBeInTheDocument();
  });
});
