import { screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { kubeRoutes, kubeRow, loggedIn, renderApp } from "@/test/app";
import { mockFetch } from "@/test/fetch";

const MI = 1024 ** 2;

function event(name: string, type: "Warning" | "Normal", reason: string, object: [string, string], lastSeen: number, count = 1) {
  return {
    ...kubeRow(name, "apps", {
      reason,
      object_kind: object[0],
      object_name: object[1],
      object_namespace: "apps",
      object_link: object[0] === "Pod" ? "pods" : "",
      message: `${reason} happened`,
      count: String(count),
      last_seen: String(lastSeen),
      source: "kubelet",
    }),
    status: type,
    health: type === "Warning" ? "warn" : "ok",
  };
}

const nodeUsage = {
  "GET /api/v1/envs/k/k8s/metrics/nodes": {
    json: {
      available: true,
      rows: [{ name: "n1", namespace: null, cpu: 1000, memory: 4096 * MI, cpu_allocatable: 4000, memory_allocatable: 8192 * MI }],
    },
  },
  "GET /api/v1/envs/k/k8s/metrics/pods": {
    json: {
      available: true,
      rows: [
        { name: "web-1", namespace: "apps", cpu: 125, memory: 64 * MI },
        { name: "db-0", namespace: "apps", cpu: 900, memory: 512 * MI },
      ],
    },
  },
};

const noMetrics = {
  "GET /api/v1/envs/k/k8s/metrics/nodes": { json: { available: false, reason: "metrics-server is not installed in this cluster", rows: [] } },
  "GET /api/v1/envs/k/k8s/metrics/pods": { json: { available: false, reason: "metrics-server is not installed in this cluster", rows: [] } },
};

describe("Kubernetes metrics and events", () => {
  it("shows the cluster's CPU and memory against what its nodes can allocate", async () => {
    mockFetch({
      ...loggedIn,
      ...kubeRoutes,
      ...nodeUsage,
      "GET /api/v1/envs/k/k8s/nodes": { json: { rows: [{ ...kubeRow("n1"), status: "Ready" }], truncated: false } },
    });
    renderApp("/envs/k");
    const cpu = await screen.findByRole("group", { name: "CPU" });
    expect(within(cpu).getByText("25%")).toBeInTheDocument();
    expect(within(cpu).getByText("of 4 cores")).toBeInTheDocument();
    expect(within(screen.getByRole("group", { name: "Memory" })).getByText("of 8.0Gi")).toBeInTheDocument();
    const busiest = await screen.findByRole("list", { name: "Busiest pods by CPU" });
    const names = within(busiest).getAllByRole("link").map((a) => a.textContent);
    expect(names).toEqual(["db-0", "web-1"]);
    expect(within(screen.getByRole("list", { name: "Nodes" })).getByText("25%")).toBeInTheDocument();
  });

  it("says when the cluster has no metrics instead of showing empty readouts", async () => {
    mockFetch({ ...loggedIn, ...kubeRoutes, ...noMetrics });
    renderApp("/envs/k");
    expect(await screen.findByText("No CPU or memory figures: metrics-server is not installed in this cluster.")).toBeInTheDocument();
    expect(screen.queryByRole("group", { name: "CPU" })).not.toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Busiest pods" })).not.toBeInTheDocument();
  });

  it("lists the newest warnings on the overview, linking to the object's events", async () => {
    mockFetch({
      ...loggedIn,
      ...kubeRoutes,
      ...noMetrics,
      "GET /api/v1/envs/k/k8s/events": {
        json: {
          rows: [
            event("a.1", "Warning", "FailedScheduling", ["Pod", "worker-2"], 100),
            event("b.1", "Normal", "Pulled", ["Pod", "web-1"], 300),
            event("c.1", "Warning", "BackOff", ["Pod", "api-7"], 200, 14),
          ],
          truncated: false,
        },
      },
    });
    renderApp("/envs/k");
    const list = await screen.findByRole("list", { name: "Recent warnings" });
    const links = within(list).getAllByRole("link");
    expect(links.map((a) => a.textContent)).toEqual(["Pod/api-7", "Pod/worker-2"]);
    expect(links[0]).toHaveAttribute("href", "/envs/k/k8s/pods/apps/api-7?tab=events");
    expect(within(list).getByText(/×14/)).toBeInTheDocument();
    expect(within(list).queryByText("Pulled")).not.toBeInTheDocument();
  });

  it("lists events with warnings first and nothing to create", async () => {
    mockFetch({
      ...loggedIn,
      ...kubeRoutes,
      "GET /api/v1/envs/k/k8s/events": {
        json: {
          rows: [event("b.1", "Normal", "Pulled", ["Pod", "web-1"], 300), event("c.1", "Warning", "BackOff", ["Pod", "api-7"], 200, 14)],
          truncated: false,
        },
      },
    });
    renderApp("/envs/k/k8s/events");
    const table = await screen.findByRole("table", { name: "Events" });
    await within(table).findByText("BackOff");
    const reasons = within(table)
      .getAllByRole("row")
      .map((r) => within(r).queryByText(/^(BackOff|Pulled)$/)?.textContent)
      .filter(Boolean);
    expect(reasons).toEqual(["BackOff", "Pulled"]);
    expect(within(table).getByRole("link", { name: "Pod/api-7" })).toHaveAttribute("href", "/envs/k/k8s/pods/apps/api-7");
    expect(screen.queryByRole("button", { name: "Create" })).not.toBeInTheDocument();
  });

  it("adds CPU and memory to the pods list when metrics-server reports them", async () => {
    mockFetch({ ...loggedIn, ...kubeRoutes, ...nodeUsage });
    renderApp("/envs/k/k8s/pods");
    const table = await screen.findByRole("table", { name: "Pods" });
    expect(await within(table).findByText("125m")).toBeInTheDocument();
    expect(within(table).getByText("64Mi")).toBeInTheDocument();
    expect(within(table).getByRole("columnheader", { name: /CPU/ })).toBeInTheDocument();
  });
});
