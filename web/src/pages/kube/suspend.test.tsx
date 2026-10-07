import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { kubeRoutes, kubeRow, loggedIn, renderApp } from "@/test/app";
import { mockFetch } from "@/test/fetch";

const failing = { ...kubeRow("api", "apps", { ready: "0/3", up_to_date: "3", available: "0" }, "bad"), status: "0/3" };
const parked = { ...kubeRow("old", "apps", { ready: "0/0", suspended_replicas: "2" }, "off"), status: "Suspended" };
const object = (row: object) => ({ row, yaml: "kind: Deployment\n", conditions: [], owners: [], containers: [] });

const routes = {
  ...loggedIn,
  ...kubeRoutes,
  "GET /api/v1/envs/k/k8s/deployments": { json: { rows: [failing, parked], truncated: false } },
  "GET /api/v1/envs/k/k8s/deployments/apps/api": { json: object(failing) },
  "GET /api/v1/envs/k/k8s/deployments/apps/api/events": { json: [] },
  "GET /api/v1/envs/k/k8s/deployments/apps/old": { json: object(parked) },
  "GET /api/v1/envs/k/k8s/deployments/apps/old/events": { json: [] },
  "POST /api/v1/envs/k/k8s/deployments/apps/api/suspend": { status: 204 },
  "POST /api/v1/envs/k/k8s/deployments/apps/old/resume": { status: 204 },
};

describe("suspending workloads", () => {
  it("suspends a deployment from its page, saying what will happen", async () => {
    const calls = mockFetch(routes);
    renderApp("/envs/k/k8s/deployments/apps/api");
    await userEvent.click(await screen.findByRole("button", { name: "Suspend" }));
    const dialog = await screen.findByRole("dialog", { name: "Suspend Deployment api?" });
    expect(dialog).toHaveTextContent("remembers its 3");
    expect(dialog).toHaveTextContent("HorizontalPodAutoscaler");
    await userEvent.click(within(dialog).getByRole("button", { name: "Suspend" }));
    await waitFor(() => expect(calls.some((c) => c.method === "POST" && c.path === "/api/v1/envs/k/k8s/deployments/apps/api/suspend")).toBe(true));
  });

  it("offers Resume, with the kept count, on a suspended deployment", async () => {
    const calls = mockFetch(routes);
    renderApp("/envs/k/k8s/deployments/apps/old");
    await userEvent.click(await screen.findByRole("button", { name: "Resume" }));
    const dialog = await screen.findByRole("dialog", { name: "Resume Deployment old?" });
    expect(dialog).toHaveTextContent("back to 2 replicas");
    expect(screen.queryByRole("button", { name: "Restart" })).not.toBeInTheDocument();
    await userEvent.click(within(dialog).getByRole("button", { name: "Resume" }));
    await waitFor(() => expect(calls.some((c) => c.path === "/api/v1/envs/k/k8s/deployments/apps/old/resume")).toBe(true));
  });

  it("racks suspended deployments apart and suspends selected ones in bulk", async () => {
    mockFetch(routes);
    renderApp("/envs/k/k8s/deployments");
    const table = await screen.findByRole("table", { name: "Deployments" });
    expect(await within(table).findByRole("rowgroup", { name: "Suspended" })).toHaveTextContent("old");
    await userEvent.click(within(table).getByRole("checkbox", { name: "Select apps/api" }));
    await userEvent.click(within(table).getByRole("checkbox", { name: "Select apps/old" }));
    // One of each: both actions are offered, each for its own rows.
    expect(screen.getByRole("button", { name: "Resume" })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Suspend" }));
    expect(await screen.findByRole("dialog", { name: "Suspend Deployment api?" })).toBeInTheDocument();
  });

  it("suspends a failing workload straight from the overview", async () => {
    const calls = mockFetch(routes);
    renderApp("/envs/k");
    const bay = await screen.findByRole("list", { name: "Needs attention" });
    await userEvent.click(within(bay).getByRole("button", { name: "Suspend deployment api" }));
    const dialog = await screen.findByRole("dialog", { name: "Suspend Deployment api?" });
    await userEvent.click(within(dialog).getByRole("button", { name: "Suspend" }));
    await waitFor(() => expect(calls.some((c) => c.path === "/api/v1/envs/k/k8s/deployments/apps/api/suspend")).toBe(true));
    // The suspended one is not something that needs attention.
    expect(within(bay).queryByText("old")).not.toBeInTheDocument();
  });
});
