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
});
