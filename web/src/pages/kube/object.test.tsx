import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { kubeRoutes, loggedIn, renderApp } from "@/test/app";
import { mockFetch } from "@/test/fetch";
import { FakeWebSocket } from "@/test/fake-ws";

const deployment = {
  row: { name: "web", namespace: "apps", created: 1, status: "2/3", health: "warn", columns: { ready: "2/3" } },
  yaml: "kind: Deployment\nmetadata:\n  name: web\n",
  conditions: [{ type: "Available", status: "True", reason: "MinimumReplicasAvailable", message: null }],
  owners: [],
  containers: [],
};
const pod = {
  row: { name: "web-1", namespace: "apps", created: 1, status: "Running", health: "ok", columns: {} },
  yaml: "kind: Pod\n",
  conditions: [],
  owners: [{ kind: "ReplicaSet", name: "web-7d9", link: null }, { kind: "Deployment", name: "web", link: "deployments" }],
  containers: ["main", "side"],
};
const routes = {
  ...loggedIn,
  ...kubeRoutes,
  "GET /api/v1/envs/k/k8s/deployments/apps/web": { json: deployment },
  "GET /api/v1/envs/k/k8s/deployments/apps/web/events": { json: [{ type: "Normal", reason: "ScalingReplicaSet", message: "Scaled up", count: 1, last_seen: 1, source: "deployment-controller" }] },
  "GET /api/v1/envs/k/k8s/pods/apps/web-1": { json: pod },
  "GET /api/v1/envs/k/k8s/pods/apps/web-1/events": { json: [] },
  "POST /api/v1/envs/k/k8s/deployments/apps/web/scale": { status: 204 },
  "POST /api/v1/envs/k/k8s/deployments/apps/web/restart": { status: 204 },
  "DELETE /api/v1/envs/k/k8s/deployments/apps/web": { status: 204 },
};

describe("a Kubernetes object's page", () => {
  it("shows the summary, YAML and events", async () => {
    mockFetch(routes);
    renderApp("/envs/k/k8s/deployments/apps/web");
    expect(await screen.findByRole("heading", { name: "web" })).toBeInTheDocument();
    expect(await screen.findByText("MinimumReplicasAvailable")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("tab", { name: /YAML/ }));
    expect(await screen.findByText(/kind: Deployment/)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("tab", { name: /Events/ }));
    expect(await screen.findByText("Scaled up")).toBeInTheDocument();
  });

  it("scales, restarts and deletes", async () => {
    const calls = mockFetch(routes);
    const { history } = renderApp("/envs/k/k8s/deployments/apps/web");
    await userEvent.click(await screen.findByRole("button", { name: "Scale" }));
    const dialog = await screen.findByRole("dialog");
    const input = within(dialog).getByLabelText("Replicas");
    await userEvent.clear(input);
    await userEvent.type(input, "5");
    await userEvent.click(within(dialog).getByRole("button", { name: "Scale" }));
    expect(calls.find((c) => c.method === "POST" && c.path.endsWith("/scale"))?.body).toEqual({ replicas: 5 });
    await userEvent.click(screen.getByRole("button", { name: "Restart" }));
    await userEvent.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "Restart" }));
    expect(calls.some((c) => c.method === "POST" && c.path.endsWith("/restart"))).toBe(true);
    await userEvent.click(screen.getByRole("button", { name: "Delete" }));
    await userEvent.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "Delete" }));
    expect(calls.some((c) => c.method === "DELETE")).toBe(true);
    await screen.findByRole("table", { name: "Deployments" });
    expect(history.location.pathname).toBe("/envs/k/k8s/deployments");
  });

  it("a pod links its owners and streams logs from the chosen container", async () => {
    mockFetch(routes);
    renderApp("/envs/k/k8s/pods/apps/web-1?tab=logs");
    expect(await screen.findByRole("link", { name: "web" })).toHaveAttribute("href", "/envs/k/k8s/deployments/apps/web");
    expect(screen.queryByRole("button", { name: "Scale" })).not.toBeInTheDocument();
    const ws = FakeWebSocket.latest();
    if (ws.readyState !== FakeWebSocket.OPEN) act(() => ws.open());
    await userEvent.selectOptions(await screen.findByRole("combobox", { name: "Container" }), "side");
    let last: Record<string, unknown> | undefined;
    await waitFor(() => {
      last = ws
        .frames()
        .filter((m) => m.op === "sub" && m.topic === "logs")
        .pop();
      expect(last?.opts).toMatchObject({ container: "side" });
    });
    expect(last).toMatchObject({ env: "k", id: "apps/web-1", opts: { container: "side" } });
  });
});
