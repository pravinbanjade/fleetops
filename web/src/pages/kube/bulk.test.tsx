import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { kubeRoutes, kubeRow, loggedIn, renderApp } from "@/test/app";
import { mockFetch } from "@/test/fetch";

const dep = (name: string) => kubeRow(name, "apps", { ready: "2/2", up_to_date: "2", available: "2" });
const object = (row: object) => ({ row, yaml: "kind: CronJob\n", conditions: [], owners: [], containers: [] });
const cron = { ...kubeRow("nightly", "apps", { schedule: "0 3 * * *", active: "0", last_schedule: "" }), status: "Scheduled" };
const nodeRow = (name: string, cordoned = false) => ({
  ...kubeRow(name, null, cordoned ? { unschedulable: "true" } : {}, cordoned ? "warn" : "ok"),
  status: cordoned ? "Ready,SchedulingDisabled" : "Ready",
});

const routes = {
  ...loggedIn,
  ...kubeRoutes,
  "GET /api/v1/envs/k/k8s/deployments": { json: { rows: [dep("web"), dep("api"), dep("db")], truncated: false } },
  "GET /api/v1/envs/k/k8s/cronjobs": { json: { rows: [cron], truncated: false } },
  "GET /api/v1/envs/k/k8s/cronjobs/apps/nightly": { json: object(cron) },
  "GET /api/v1/envs/k/k8s/cronjobs/apps/nightly/events": { json: [] },
  "GET /api/v1/envs/k/k8s/nodes": { json: { rows: [nodeRow("n1"), nodeRow("n2", true)], truncated: false } },
  "POST /api/v1/envs/k/k8s/deployments/apps/web/restart": { status: 204 },
  "POST /api/v1/envs/k/k8s/deployments/apps/api/restart": { status: 204 },
  "POST /api/v1/envs/k/k8s/deployments/apps/web/scale": { status: 204 },
  "POST /api/v1/envs/k/k8s/deployments/apps/api/scale": { status: 204 },
  "DELETE /api/v1/envs/k/k8s/pods/apps/web-1": { status: 204 },
  "POST /api/v1/envs/k/k8s/nodes/-/n1/cordon": { status: 204 },
  "POST /api/v1/envs/k/k8s/nodes/-/n2/uncordon": { status: 204 },
  "POST /api/v1/envs/k/k8s/cronjobs/apps/nightly/trigger": { json: { job: "nightly-manual-42" } },
  "GET /api/v1/envs/k/k8s/jobs/apps/nightly-manual-42": { json: { ...object(kubeRow("nightly-manual-42", "apps")), yaml: "kind: Job\n" } },
  "GET /api/v1/envs/k/k8s/jobs/apps/nightly-manual-42/events": { json: [] },
};

const posted = (calls: { method: string; path: string }[], path: string) => calls.some((c) => c.method !== "GET" && c.path === path);
const select = async (table: HTMLElement, ...ids: string[]) => {
  for (const id of ids) await userEvent.click(within(table).getByRole("checkbox", { name: `Select ${id}` }));
};

describe("acting on a selection", () => {
  it("restarts the selected deployments, one after another", async () => {
    const calls = mockFetch(routes);
    renderApp("/envs/k/k8s/deployments");
    const table = await screen.findByRole("table", { name: "Deployments" });
    await within(table).findByText("web");
    await select(table, "apps/web", "apps/api");
    const bar = screen.getByRole("group", { name: "Selected" });
    expect(within(bar).getAllByRole("button").map((b) => b.textContent)).toEqual(["Restart 2", "Scale 2", "Suspend 2", "Delete 2"]);
    await userEvent.click(within(bar).getByRole("button", { name: "Restart 2" }));
    const dialog = await screen.findByRole("dialog", { name: "Restart 2 Deployments?" });
    await userEvent.click(within(dialog).getByRole("button", { name: "Restart" }));
    await waitFor(() => expect(posted(calls, "/api/v1/envs/k/k8s/deployments/apps/api/restart")).toBe(true));
    expect(posted(calls, "/api/v1/envs/k/k8s/deployments/apps/web/restart")).toBe(true);
    expect(posted(calls, "/api/v1/envs/k/k8s/deployments/apps/db/restart")).toBe(false);
    await waitFor(() => expect(screen.queryByRole("group", { name: "Selected" })).not.toBeInTheDocument());
  });

  it("scales a selection to one replica count", async () => {
    const calls = mockFetch(routes);
    renderApp("/envs/k/k8s/deployments");
    const table = await screen.findByRole("table", { name: "Deployments" });
    await within(table).findByText("web");
    await select(table, "apps/web", "apps/api");
    await userEvent.click(screen.getByRole("button", { name: "Scale 2" }));
    const dialog = await screen.findByRole("dialog", { name: "Scale 2 Deployments" });
    await userEvent.type(within(dialog).getByRole("spinbutton", { name: "Replicas" }), "4");
    await userEvent.click(within(dialog).getByRole("button", { name: "Scale" }));
    await waitFor(() => expect(posted(calls, "/api/v1/envs/k/k8s/deployments/apps/api/scale")).toBe(true));
    const body = calls.find((c) => c.path === "/api/v1/envs/k/k8s/deployments/apps/web/scale")?.body;
    expect(body).toEqual({ replicas: 4 });
  });

  it("says how far a bulk action got when one fails, and resumes from there", async () => {
    const calls = mockFetch({
      ...routes,
      "POST /api/v1/envs/k/k8s/deployments/apps/web/restart": { status: 403, json: { error: "forbidden", message: "no permission" } },
    });
    renderApp("/envs/k/k8s/deployments");
    const table = await screen.findByRole("table", { name: "Deployments" });
    await within(table).findByText("web");
    await select(table, "apps/api", "apps/web");
    await userEvent.click(screen.getByRole("button", { name: "Restart 2" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.click(within(dialog).getByRole("button", { name: "Restart" }));
    expect(await within(dialog).findByText(/^1 of 2 done\. web:/)).toBeInTheDocument();
    await userEvent.click(within(dialog).getByRole("button", { name: "Restart" }));
    await within(dialog).findByText(/^1 of 2 done\. web:/);
    // api went once: trying again did not restart it a second time.
    expect(calls.filter((c) => c.path === "/api/v1/envs/k/k8s/deployments/apps/api/restart")).toHaveLength(1);
  });

  it("deletes selected pods behind a red confirmation", async () => {
    const calls = mockFetch(routes);
    renderApp("/envs/k/k8s/pods");
    const table = await screen.findByRole("table", { name: "Pods" });
    await within(table).findByText("web-1");
    await select(table, "apps/web-1");
    const bar = screen.getByRole("group", { name: "Selected" });
    expect(within(bar).getAllByRole("button").map((b) => b.textContent)).toEqual(["Delete"]);
    await userEvent.click(within(bar).getByRole("button", { name: "Delete" }));
    const dialog = await screen.findByRole("dialog", { name: "Delete Pod web-1?" });
    expect(dialog).toHaveTextContent("replaced by a new one");
    await userEvent.click(within(dialog).getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(posted(calls, "/api/v1/envs/k/k8s/pods/apps/web-1")).toBe(true));
  });

  it("cordons and uncordons nodes, each for the ones it fits", async () => {
    const calls = mockFetch(routes);
    renderApp("/envs/k/k8s/nodes");
    const table = await screen.findByRole("table", { name: "Nodes" });
    expect(await within(table).findByText("Ready,SchedulingDisabled")).toBeInTheDocument();
    await select(table, "-/n1", "-/n2");
    const bar = screen.getByRole("group", { name: "Selected" });
    expect(within(bar).getAllByRole("button").map((b) => b.textContent)).toEqual(["Cordon", "Uncordon", "Delete 2"]);
    await userEvent.click(within(bar).getByRole("button", { name: "Cordon" }));
    const dialog = await screen.findByRole("dialog", { name: "Cordon Node n1?" });
    expect(dialog).toHaveTextContent("Pods already there keep running");
    await userEvent.click(within(dialog).getByRole("button", { name: "Cordon" }));
    await waitFor(() => expect(posted(calls, "/api/v1/envs/k/k8s/nodes/-/n1/cordon")).toBe(true));
    expect(posted(calls, "/api/v1/envs/k/k8s/nodes/-/n2/cordon")).toBe(false);
  });

  it("runs a CronJob now from its page and opens the Job it started", async () => {
    const calls = mockFetch(routes);
    renderApp("/envs/k/k8s/cronjobs/apps/nightly");
    await userEvent.click(await screen.findByRole("button", { name: "Run now" }));
    const dialog = await screen.findByRole("dialog", { name: "Run now CronJob nightly?" });
    await userEvent.click(within(dialog).getByRole("button", { name: "Run now" }));
    await waitFor(() => expect(posted(calls, "/api/v1/envs/k/k8s/cronjobs/apps/nightly/trigger")).toBe(true));
    expect(await screen.findByRole("heading", { name: "nightly-manual-42" })).toBeInTheDocument();
  });

  it("offers no selection where nothing can be done", async () => {
    mockFetch({ ...routes, "GET /api/v1/envs/k/k8s/events": { json: { rows: [], truncated: false } } });
    renderApp("/envs/k/k8s/events");
    const table = await screen.findByRole("table", { name: "Events" });
    expect(within(table).queryByRole("checkbox")).not.toBeInTheDocument();
  });
});
