import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { KUBE_ENV, kubeRoutes, kubeRow, loggedIn, renderApp } from "@/test/app";
import { ENVS } from "@/test/app";
import { mockFetch } from "@/test/fetch";

const pod = (name: string, status = "Running") => ({ ...kubeRow(name, "apps", { ready: "1/1", restarts: "0" }), status });
const routes = {
  ...loggedIn,
  ...kubeRoutes,
  "GET /api/v1/envs/k/k8s/pods": { json: { rows: [pod("web-1"), pod("job-1", "Succeeded")], truncated: false } },
  "GET /api/v1/envs/k/k8s/pods/apps/web-1": { json: { row: pod("web-1"), yaml: "kind: Pod\\n", conditions: [], owners: [], containers: ["app"] } },
  "GET /api/v1/envs/k/k8s/pods/apps/web-1/events": { json: [] },
};

describe("quick actions on a kube list", () => {
  it("offers one selected pod its logs, shell, events and YAML beside Delete", async () => {
    mockFetch(routes);
    const { history } = renderApp("/envs/k/k8s/pods");
    const table = await screen.findByRole("table", { name: "Pods" });
    await userEvent.click(await within(table).findByRole("checkbox", { name: "Select apps/web-1" }));
    const bar = screen.getByRole("group", { name: "Selected" });
    expect(within(bar).getAllByRole("button").map((b) => b.textContent)).toEqual(["Logs", "Terminal", "Events", "Edit YAML", "Delete"]);
    // A second pick leaves only what applies to both.
    await userEvent.click(within(table).getByRole("checkbox", { name: "Select apps/job-1" }));
    expect(within(bar).getAllByRole("button").map((b) => b.textContent)).toEqual(["Delete 2"]);
    await userEvent.click(within(table).getByRole("checkbox", { name: "Select apps/job-1" }));
    await userEvent.click(within(bar).getByRole("button", { name: "Logs" }));
    await waitFor(() => expect(history.location.pathname).toBe("/envs/k/k8s/pods/apps/web-1"));
    expect(history.location.search).toContain("tab=logs");
  });

  it("puts each row's actions in its menu", async () => {
    mockFetch(routes);
    const { history } = renderApp("/envs/k/k8s/pods");
    await userEvent.click(await screen.findByRole("button", { name: "Actions for web-1" }));
    const menu = await screen.findByRole("menu");
    expect(within(menu).getAllByRole("menuitem").map((m) => m.textContent)).toEqual(["Logs", "Terminal", "Events", "Edit YAML", "Delete"]);
    await userEvent.click(within(menu).getByRole("menuitem", { name: "Terminal" }));
    await waitFor(() => expect(history.location.search).toContain("tab=terminal"));
  });

  it("asks before deleting from a row's menu", async () => {
    mockFetch(routes);
    renderApp("/envs/k/k8s/pods");
    await userEvent.click(await screen.findByRole("button", { name: "Actions for job-1" }));
    await userEvent.click(within(await screen.findByRole("menu")).getByRole("menuitem", { name: "Delete" }));
    expect(await screen.findByRole("dialog", { name: "Delete Pod job-1?" })).toBeInTheDocument();
  });

  it("gives a viewer a menu of what they can read", async () => {
    mockFetch({ ...routes, "GET /api/v1/envs": { json: [...ENVS, { ...KUBE_ENV, access: { role: "viewer", namespaces: null } }] } });
    renderApp("/envs/k/k8s/pods");
    await userEvent.click(await screen.findByRole("button", { name: "Actions for web-1" }));
    const menu = await screen.findByRole("menu");
    expect(within(menu).getAllByRole("menuitem").map((m) => m.textContent)).toEqual(["Logs", "Events"]);
  });
});
