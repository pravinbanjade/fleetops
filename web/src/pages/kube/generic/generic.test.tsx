import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { WIDGETS, genericRoutes, kubeRoutes, loggedIn, renderApp } from "@/test/app";
import { mockFetch } from "@/test/fetch";

describe("custom resources", () => {
  it("lists a discovered kind with its printer columns", async () => {
    mockFetch({ ...loggedIn, ...kubeRoutes, ...genericRoutes });
    renderApp("/envs/k/k8s/r/example.fleetops.io/v1/widgets");
    const table = await screen.findByRole("table", { name: "Widget" });
    expect(within(table).getByRole("columnheader", { name: /Size/ })).toBeInTheDocument();
    expect(within(table).getByRole("columnheader", { name: /Expires/ })).toBeInTheDocument();
    expect(await within(table).findByRole("link", { name: "w1" })).toHaveAttribute(
      "href",
      "/envs/k/k8s/r/example.fleetops.io/v1/widgets/apps/w1",
    );
    expect(within(table).getByText("3")).toBeInTheDocument();
  });

  it("opens one with conditions and its YAML", async () => {
    mockFetch({ ...loggedIn, ...kubeRoutes, ...genericRoutes });
    renderApp("/envs/k/k8s/r/example.fleetops.io/v1/widgets/apps/w1");
    expect(await screen.findByRole("heading", { name: "w1" })).toBeInTheDocument();
    expect(await screen.findByRole("region", { name: "Conditions" })).toBeInTheDocument();
    expect(await screen.findByRole("button", { name: "Edit YAML" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Delete/ })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("tab", { name: /YAML/ }));
    expect(screen.getByLabelText("Object YAML")).toHaveTextContent("kind: Widget");
  });

  it("hides Edit and Delete when the verbs do not allow them", async () => {
    mockFetch({
      ...loggedIn,
      ...kubeRoutes,
      ...genericRoutes,
      "GET /api/v1/envs/k/k8s/discovery": { json: { kinds: [{ ...WIDGETS, verbs: ["get", "list", "watch"] }], failed_groups: [] } },
    });
    renderApp("/envs/k/k8s/r/example.fleetops.io/v1/widgets/apps/w1");
    await screen.findByRole("region", { name: "Conditions" });
    expect(screen.queryByRole("button", { name: "Edit YAML" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Delete/ })).not.toBeInTheDocument();
  });

  it("puts a custom resource's events, YAML and Delete in its row menu", async () => {
    mockFetch({ ...loggedIn, ...kubeRoutes, ...genericRoutes });
    const { history } = renderApp("/envs/k/k8s/r/example.fleetops.io/v1/widgets");
    await userEvent.click(await screen.findByRole("button", { name: "Actions for w1" }));
    const menu = await screen.findByRole("menu");
    expect(within(menu).getAllByRole("menuitem").map((m) => m.textContent)).toEqual(["Events", "Edit YAML", "Delete"]);
    await userEvent.click(within(menu).getByRole("menuitem", { name: "Events" }));
    expect(history.location.pathname).toBe("/envs/k/k8s/r/example.fleetops.io/v1/widgets/apps/w1");
    expect(history.location.search).toContain("tab=events");
  });

  it("leaves only Events in the row menu when the verbs allow no change", async () => {
    mockFetch({
      ...loggedIn,
      ...kubeRoutes,
      ...genericRoutes,
      "GET /api/v1/envs/k/k8s/discovery": { json: { kinds: [{ ...WIDGETS, verbs: ["get", "list", "watch"] }], failed_groups: [] } },
    });
    renderApp("/envs/k/k8s/r/example.fleetops.io/v1/widgets");
    await userEvent.click(await screen.findByRole("button", { name: "Actions for w1" }));
    expect(within(await screen.findByRole("menu")).getAllByRole("menuitem").map((m) => m.textContent)).toEqual(["Events"]);
  });

  it("says when the kind no longer exists", async () => {
    mockFetch({
      ...loggedIn,
      ...kubeRoutes,
      ...genericRoutes,
      "GET /api/v1/envs/k/k8s/r/nope.io/v1/things": {
        status: 404,
        json: { code: "not_found", message: "the cluster does not serve nope.io/v1/things", request_id: null },
      },
    });
    renderApp("/envs/k/k8s/r/nope.io/v1/things");
    expect(await screen.findByText(/This resource type no longer exists/)).toBeInTheDocument();
  });

  it("says when the credentials cannot list the kind", async () => {
    mockFetch({
      ...loggedIn,
      ...kubeRoutes,
      ...genericRoutes,
      "GET /api/v1/envs/k/k8s/r/example.fleetops.io/v1/widgets": {
        status: 403,
        json: { code: "forbidden", message: "widgets is forbidden", request_id: null },
      },
    });
    renderApp("/envs/k/k8s/r/example.fleetops.io/v1/widgets");
    expect(await screen.findByText(/credentials can't list Widget/)).toBeInTheDocument();
  });

  it("lists discovered kinds under Custom Resources, by API group", async () => {
    mockFetch({ ...loggedIn, ...kubeRoutes, ...genericRoutes });
    renderApp("/envs/k");
    const nav = await screen.findByRole("navigation", { name: "Environment" });
    await userEvent.click(await within(nav).findByRole("button", { name: /Custom Resources/ }));
    expect(within(nav).getByRole("link", { name: "Definitions" })).toHaveAttribute(
      "href",
      "/envs/k/k8s/r/apiextensions.k8s.io/v1/customresourcedefinitions",
    );
    await userEvent.click(within(nav).getByRole("button", { name: /example\.fleetops\.io/ }));
    expect(within(nav).getByRole("link", { name: "Widget" })).toHaveAttribute("href", "/envs/k/k8s/r/example.fleetops.io/v1/widgets");
    expect(within(nav).getByRole("status")).toHaveTextContent("Couldn't load: metrics.k8s.io");
  });

  it("says when discovery fails and offers a retry, leaving curated kinds alone", async () => {
    mockFetch({
      ...loggedIn,
      ...kubeRoutes,
      "GET /api/v1/envs/k/k8s/discovery": { status: 502, json: { code: "upstream", message: "down", request_id: null } },
    });
    renderApp("/envs/k");
    const nav = await screen.findByRole("navigation", { name: "Environment" });
    expect(await within(nav).findByText(/Couldn't discover API resources/, {}, { timeout: 4000 })).toBeInTheDocument();
    expect(within(nav).getByRole("button", { name: "Retry" })).toBeInTheDocument();
    expect(within(nav).getByRole("button", { name: /Workloads/ })).toBeInTheDocument();
  });

  it("refreshes discovery for a kind installed after the browser read it", async () => {
    mockFetch({
      ...loggedIn,
      ...kubeRoutes,
      ...genericRoutes,
      // The cached answer predates the CRD; only a refresh knows it.
      "GET /api/v1/envs/k/k8s/discovery": (c) => ({
        json: { kinds: c.path.includes("refresh=1") ? [WIDGETS] : [], failed_groups: [] },
      }),
    });
    renderApp("/envs/k/k8s/r/example.fleetops.io/v1/widgets");
    const table = await screen.findByRole("table", { name: "Widget" });
    expect(await within(table).findByRole("columnheader", { name: /Size/ })).toBeInTheDocument();
  });

  it("shows no Definitions link when the grant cannot read CRDs", async () => {
    mockFetch({
      ...loggedIn,
      ...kubeRoutes,
      ...genericRoutes,
      "GET /api/v1/envs/k/k8s/discovery": { json: { kinds: [WIDGETS], failed_groups: [] } },
    });
    renderApp("/envs/k");
    const nav = await screen.findByRole("navigation", { name: "Environment" });
    await userEvent.click(await within(nav).findByRole("button", { name: /Custom Resources/ }));
    expect(within(nav).getByRole("button", { name: /example\.fleetops\.io/ })).toBeInTheDocument();
    expect(within(nav).queryByRole("link", { name: "Definitions" })).not.toBeInTheDocument();
  });

  it("links an owner FleetOps has a page for", async () => {
    const obj = genericRoutes["GET /api/v1/envs/k/k8s/r/example.fleetops.io/v1/widgets/apps/w1"].json;
    mockFetch({
      ...loggedIn,
      ...kubeRoutes,
      ...genericRoutes,
      "GET /api/v1/envs/k/k8s/r/example.fleetops.io/v1/widgets/apps/w1": {
        json: { ...obj, owners: [{ kind: "Deployment", name: "web", link: "deployments" }, { kind: "Gadget", name: "g1", link: null }] },
      },
    });
    renderApp("/envs/k/k8s/r/example.fleetops.io/v1/widgets/apps/w1");
    expect(await screen.findByRole("link", { name: "web" })).toHaveAttribute("href", "/envs/k/k8s/deployments/apps/web");
    expect(screen.getByText(/g1/)).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "g1" })).not.toBeInTheDocument();
  });

  it("finds a discovered kind from the command palette", async () => {
    mockFetch({ ...loggedIn, ...kubeRoutes, ...genericRoutes });
    renderApp("/envs/k");
    await screen.findByRole("navigation", { name: "Environment" });
    await userEvent.keyboard("{Control>}k{/Control}");
    const dialog = await screen.findByRole("dialog");
    await userEvent.type(within(dialog).getByRole("combobox"), "Widget");
    expect(await within(dialog).findByRole("option", { name: /Widget \(example\.fleetops\.io\)/ })).toBeInTheDocument();
  });
});
