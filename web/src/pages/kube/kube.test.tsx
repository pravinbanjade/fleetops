import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { kubeRoutes, kubeRow, loggedIn, renderApp } from "@/test/app";
import { mockFetch } from "@/test/fetch";

describe("Kubernetes environments", () => {
  it("shows the Kubernetes pages in the sidebar, grouped and folded", async () => {
    mockFetch({ ...loggedIn, ...kubeRoutes });
    renderApp("/envs/k");
    const nav = await screen.findByRole("navigation", { name: "Environment" });
    for (const label of ["Overview", "Events", "Helm releases"]) {
      expect(within(nav).getByRole("link", { name: new RegExp(label) })).toBeInTheDocument();
    }
    // Groups start folded on the overview; opening one shows its pages.
    for (const group of ["Workloads", "Network", "Config", "Cluster"]) {
      expect(within(nav).getByRole("button", { name: new RegExp(group) })).toHaveAttribute("aria-expanded", "false");
    }
    expect(within(nav).queryByRole("link", { name: /Pods/ })).not.toBeInTheDocument();
    await userEvent.click(within(nav).getByRole("button", { name: /Workloads/ }));
    for (const label of ["Pods", "Deployments", "StatefulSets", "DaemonSets", "Jobs", "CronJobs"]) {
      expect(within(nav).getByRole("link", { name: label })).toBeInTheDocument();
    }
    await userEvent.click(within(nav).getByRole("button", { name: /Cluster/ }));
    expect(within(nav).getByRole("link", { name: /Nodes/ })).toHaveAttribute("href", "/envs/k/k8s/nodes");
    expect(within(nav).queryByRole("link", { name: /Containers/ })).not.toBeInTheDocument();
    expect(await screen.findByText("v1.32.0")).toBeInTheDocument();
    expect(screen.getByText("3 of 3 nodes ready")).toBeInTheDocument();
  });

  it("opens the group of the page shown", async () => {
    mockFetch({ ...loggedIn, ...kubeRoutes });
    renderApp("/envs/k/k8s/secrets");
    const nav = await screen.findByRole("navigation", { name: "Environment" });
    expect(within(nav).getByRole("button", { name: /Config/ })).toHaveAttribute("aria-expanded", "true");
    expect(within(nav).getByRole("link", { name: /Secrets/ })).toHaveAttribute("data-status", "active");
    expect(within(nav).getByRole("button", { name: /Workloads/ })).toHaveAttribute("aria-expanded", "false");
  });

  it("puts what needs attention first on the overview", async () => {
    mockFetch({
      ...loggedIn,
      ...kubeRoutes,
      "GET /api/v1/envs/k/k8s/pods": {
        json: {
          rows: [
            kubeRow("web-1", "apps", { ready: "1/1", restarts: "0" }),
            { ...kubeRow("api-7", "apps", { ready: "0/1", restarts: "4" }, "bad"), status: "CrashLoopBackOff" },
          ],
          truncated: false,
        },
      },
    });
    renderApp("/envs/k");
    const bay = await screen.findByRole("list", { name: "Needs attention" });
    expect(within(bay).getByRole("link", { name: "api-7" })).toHaveAttribute("href", "/envs/k/k8s/pods/apps/api-7?tab=logs");
    expect(within(bay).getByText("4 restarts")).toBeInTheDocument();
    // The deployment short of replicas is listed too.
    expect(within(bay).getByRole("link", { name: "web" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "2 things need attention" })).toBeInTheDocument();
    expect(within(screen.getByRole("list", { name: "Pods by status" })).getByRole("link", { name: /CrashLoopBackOff/ })).toHaveAttribute(
      "href",
      "/envs/k/k8s/pods?q=CrashLoopBackOff",
    );
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
    // The other workload kinds sit beside it in the sidebar.
    expect(screen.getByRole("link", { name: "StatefulSets" })).toHaveAttribute("href", "/envs/k/k8s/statefulsets");
  });

  it("asks for one namespace once it is picked, and remembers it", async () => {
    const calls = mockFetch({ ...loggedIn, ...kubeRoutes, "GET /api/v1/envs/k/k8s/pods": { json: { rows: [], truncated: false } } });
    renderApp("/envs/k/k8s/pods");
    await userEvent.click(await screen.findByRole("combobox", { name: "Namespace" }));
    await userEvent.type(screen.getByRole("combobox", { name: "Search namespaces" }), "ap");
    // The search narrows the list; "default" no longer matches.
    expect(screen.queryByRole("option", { name: "default" })).not.toBeInTheDocument();
    await userEvent.click(await screen.findByRole("option", { name: "apps" }));
    expect(screen.getByRole("combobox", { name: "Namespace" })).toHaveTextContent("apps");
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
    expect(await screen.findByText("Nodes not visible to these credentials")).toBeInTheDocument();
  });
});

describe("credentials that cannot list namespaces", () => {
  it("let the user type the namespace to show", async () => {
    const calls = mockFetch({
      ...loggedIn,
      ...kubeRoutes,
      "GET /api/v1/envs/k/k8s/namespaces": { status: 403, json: { code: "forbidden", message: "namespaces is forbidden", request_id: null } },
    });
    renderApp("/envs/k/k8s/pods");
    await userEvent.click(await screen.findByRole("combobox", { name: "Namespace" }));
    await screen.findByText(/may not list namespaces/);
    await userEvent.type(screen.getByRole("combobox", { name: "Search namespaces" }), "apps{Enter}");
    await screen.findByRole("table", { name: "Pods" });
    await waitFor(() => expect(calls.some((c) => c.path === "/api/v1/envs/k/k8s/pods?namespace=apps")).toBe(true));
  });

  it("opens Apply YAML from the top bar of a Kubernetes environment", async () => {
    mockFetch({ ...loggedIn, ...kubeRoutes });
    renderApp("/envs/k/k8s/pods");
    await userEvent.click(await screen.findByRole("button", { name: "Apply YAML" }));
    expect(await screen.findByRole("dialog", { name: "Apply YAML" })).toBeInTheDocument();
  });

  it("creates from a template on a list", async () => {
    mockFetch({
      ...loggedIn,
      ...kubeRoutes,
      "GET /api/v1/envs/k/k8s/templates": { json: { deployments: "kind: Deployment\nmetadata:\n  namespace: NAMESPACE\n" } },
      "GET /api/v1/envs/k/k8s/nodes": { json: { rows: [], truncated: false } },
    });
    renderApp("/envs/k/k8s/deployments");
    await userEvent.click(await screen.findByRole("button", { name: "Create" }));
    expect(await screen.findByRole("dialog", { name: "Create Deployment" })).toBeInTheDocument();
  });

  it("offers Edit YAML on an object page", async () => {
    mockFetch({
      ...loggedIn,
      ...kubeRoutes,
      "GET /api/v1/envs/k/k8s/deployments/apps/web": {
        json: { row: kubeRow("web", "apps"), yaml: "kind: Deployment\n", conditions: [], owners: [], containers: [] },
      },
      "GET /api/v1/envs/k/k8s/deployments/apps/web/edit": { json: { yaml: "kind: Deployment\n", resource_version: "1" } },
    });
    renderApp("/envs/k/k8s/deployments/apps/web");
    await userEvent.click(await screen.findByRole("button", { name: "Edit YAML" }));
    expect(await screen.findByRole("dialog", { name: "Edit web" })).toBeInTheDocument();
  });
});
