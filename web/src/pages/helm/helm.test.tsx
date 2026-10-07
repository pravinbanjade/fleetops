import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { KUBE_ENV, kubeRoutes, loggedIn, renderApp } from "@/test/app";
import { mockFetch, type Call } from "@/test/fetch";

const demo = { name: "demo", namespace: "apps", revision: 2, status: "deployed", chart: "demo", chart_version: "0.2.0", app_version: "1.1.0", updated: 1, error: null };
const broken = { ...demo, name: "broken", status: "unreadable", chart: "", chart_version: "", app_version: "", error: "not gzip" };
const detail = { release: demo, description: "Upgrade complete", notes: "Visit http://demo", values: "db:\n  password: ••••\n", manifest: "---\nkind: ConfigMap\n" };
const history = [
  { revision: 1, status: "superseded", chart_version: "0.1.0", app_version: "1.0.0", description: "Install complete", updated: 1 },
  { revision: 2, status: "deployed", chart_version: "0.2.0", app_version: "1.1.0", description: "Upgrade complete", updated: 2 },
];
const plan = (extra = {}) => ({
  current: 2,
  revision: null,
  status: null,
  note: "Chart hooks are not run by FleetOps.",
  changes: [{ api_version: "v1", kind: "ConfigMap", namespace: "apps", name: "demo-extra", change: "delete", before: "data:\n  k: v\n", after: null, error: null }],
  ...extra,
});
const helmRoutes = {
  "GET /api/v1/envs/k/k8s/helm/releases": { json: [demo, broken] },
  "GET /api/v1/envs/k/k8s/helm/releases/apps/demo": { json: detail },
  "GET /api/v1/envs/k/k8s/helm/releases/apps/demo/history": { json: history },
};
const dry = (c: Call) => (c.body as { dry_run: boolean }).dry_run;

describe("Helm releases", () => {
  it("lists releases with their status, flagging unreadable ones", async () => {
    mockFetch({ ...loggedIn, ...kubeRoutes, ...helmRoutes });
    renderApp("/envs/k/k8s/helm");
    const table = await screen.findByRole("table", { name: "Helm releases" });
    expect(await within(table).findByRole("link", { name: "demo" })).toHaveAttribute("href", "/envs/k/k8s/helm/apps/demo");
    expect(within(table).getByText("deployed")).toBeInTheDocument();
    expect(within(table).getByText("unreadable").closest("[title]")).toHaveAttribute("title", "not gzip");
    expect(within(table).getByText("demo-0.2.0")).toBeInTheDocument();
  });

  it("shows masked values and the notes", async () => {
    mockFetch({ ...loggedIn, ...kubeRoutes, ...helmRoutes });
    renderApp("/envs/k/k8s/helm/apps/demo?tab=values");
    expect(await screen.findByText(/password: ••••/)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("tab", { name: "Notes" }));
    expect(await screen.findByText("Visit http://demo")).toBeInTheDocument();
  });

  it("rolls back after showing the plan, sending the reviewed revision", async () => {
    const calls = mockFetch({
      ...loggedIn,
      ...kubeRoutes,
      ...helmRoutes,
      "POST /api/v1/envs/k/k8s/helm/releases/apps/demo/rollback": (c: Call) => ({
        json: dry(c) ? plan() : plan({ revision: 3, status: "deployed" }),
      }),
    });
    renderApp("/envs/k/k8s/helm/apps/demo?tab=history");
    await userEvent.click(await screen.findByRole("button", { name: "Roll back to 1" }));
    const dialog = await screen.findByRole("dialog");
    expect(await within(dialog).findByText("Chart hooks are not run by FleetOps.")).toBeInTheDocument();
    expect(within(dialog).getByText(/delete ConfigMap demo-extra/i)).toBeInTheDocument();
    await userEvent.click(within(dialog).getByRole("button", { name: "Roll back" }));
    expect(await within(dialog).findByText(/Revision 3 is deployed/)).toBeInTheDocument();
    const commit = calls.find((c) => c.method === "POST" && !dry(c));
    expect(commit?.body).toEqual({ revision: 1, expected_current: 2, dry_run: false });
  });

  it("asks for the release name before uninstalling", async () => {
    mockFetch({
      ...loggedIn,
      ...kubeRoutes,
      ...helmRoutes,
      "POST /api/v1/envs/k/k8s/helm/releases/apps/demo/uninstall": (c: Call) => ({
        json: dry(c) ? plan() : plan({ status: "uninstalled" }),
      }),
    });
    renderApp("/envs/k/k8s/helm/apps/demo");
    await userEvent.click(await screen.findByRole("button", { name: "Uninstall" }));
    const dialog = await screen.findByRole("dialog");
    await within(dialog).findByText("Chart hooks are not run by FleetOps.");
    const confirm = within(dialog).getByRole("button", { name: "Uninstall" });
    expect(confirm).toBeDisabled();
    await userEvent.type(within(dialog).getByLabelText(/Type demo to confirm/), "demo");
    expect(confirm).toBeEnabled();
  });

  it("says when the release changed and offers to review again", async () => {
    mockFetch({
      ...loggedIn,
      ...kubeRoutes,
      ...helmRoutes,
      "POST /api/v1/envs/k/k8s/helm/releases/apps/demo/rollback": (c: Call) =>
        dry(c) ? { json: plan() } : { status: 409, json: { code: "conflict", message: "conflict: release changed; review again" } },
    });
    renderApp("/envs/k/k8s/helm/apps/demo?tab=history");
    await userEvent.click(await screen.findByRole("button", { name: "Roll back to 1" }));
    const dialog = await screen.findByRole("dialog");
    await within(dialog).findByText("Chart hooks are not run by FleetOps.");
    await userEvent.click(within(dialog).getByRole("button", { name: "Roll back" }));
    expect(await within(dialog).findByText(/release changed; review again/)).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "Review again" })).toBeInTheDocument();
  });

  it("needs an explicit choice to commit a plan with errors", async () => {
    const failing = plan();
    failing.changes[0] = { ...failing.changes[0], error: { code: "forbidden", message: "configmaps is forbidden" } } as never;
    mockFetch({
      ...loggedIn,
      ...kubeRoutes,
      ...helmRoutes,
      "POST /api/v1/envs/k/k8s/helm/releases/apps/demo/rollback": () => ({ json: failing }),
    });
    renderApp("/envs/k/k8s/helm/apps/demo?tab=history");
    await userEvent.click(await screen.findByRole("button", { name: "Roll back to 1" }));
    const dialog = await screen.findByRole("dialog");
    expect(await within(dialog).findByText("configmaps is forbidden")).toBeInTheDocument();
    const confirm = within(dialog).getByRole("button", { name: "Roll back" });
    expect(confirm).toBeDisabled();
    await userEvent.click(within(dialog).getByRole("checkbox", { name: /1 object will fail/ }));
    expect(confirm).toBeEnabled();
  });

  it("hides rollback and uninstall from viewers", async () => {
    mockFetch({
      ...loggedIn,
      ...kubeRoutes,
      ...helmRoutes,
      "GET /api/v1/envs": { json: [{ ...KUBE_ENV, access: { role: "viewer", namespaces: null } }] },
    });
    renderApp("/envs/k/k8s/helm/apps/demo?tab=history");
    await screen.findByText("Upgrade complete", { selector: "td" });
    expect(screen.queryByRole("button", { name: /Roll back/ })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Uninstall" })).not.toBeInTheDocument();
  });

  it("shows Helm releases in the Kubernetes sidebar", async () => {
    mockFetch({ ...loggedIn, ...kubeRoutes, ...helmRoutes });
    renderApp("/envs/k");
    const nav = await screen.findByRole("navigation", { name: "Environment" });
    await waitFor(() => expect(within(nav).getByRole("link", { name: /Helm releases/ })).toHaveAttribute("href", "/envs/k/k8s/helm"));
  });
});
