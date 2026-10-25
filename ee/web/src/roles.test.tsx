import { waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { User } from "@/api/gen/types.gen";
import { mockFetch } from "@/test/fetch";
import { ADMIN, loggedIn, renderApp } from "@/test/app";
import type { Catalog, RoleSummary } from "./roles";

const BOB: User = { id: 2, username: "bob", role: "user", created_at: 0, updated_at: 0 };
const DEPLOYER: RoleSummary = {
  id: 5,
  name: "Deployer",
  description: "ship, never delete",
  permissions: ["containers.operate", "containers.view"],
  users: 1,
  teams: 0,
};
const CATALOG: Catalog = {
  rows: [
    { key: "containers", label: "Containers", group: "docker", cells: ["view", "create", "operate", "delete", "shell", "reveal"] },
    { key: "volumes", label: "Volumes", group: "docker", cells: ["view", "create", "delete"] },
    { key: "pods", label: "Pods", group: "kubernetes", cells: ["view", "create", "operate", "delete", "shell"] },
    { key: "k8s-other", label: "Everything else", group: "kubernetes", cells: ["view", "create", "operate", "delete"] },
    { key: "swarm-configs", label: "Configs", group: "swarm", cells: ["view", "create", "delete", "reveal"] },
  ],
  verbs: [
    { key: "view", label: "View" },
    { key: "create", label: "Create" },
    { key: "operate", label: "Operate" },
    { key: "delete", label: "Delete" },
    { key: "shell", label: "Shell" },
    { key: "reveal", label: "Reveal" },
  ],
};

const routes = (features: string[]) => ({
  ...loggedIn,
  "GET /api/v1/edition": { json: { edition: "enterprise", features } },
  "GET /api/v1/users": { json: [ADMIN.user, BOB] },
  "GET /api/v1/ee/roles": { json: [DEPLOYER] },
  "GET /api/v1/ee/roles/catalog": { json: CATALOG },
});

const table = (screen: typeof import("@testing-library/react").screen) => screen.findByRole("table", { name: "Roles" });
const rowOf = async (screen: typeof import("@testing-library/react").screen, name: string) =>
  within(await table(screen))
    .getAllByRole("row")
    .find((r) => r.textContent?.includes(name))!;

describe("roles", () => {
  it("lists the built-in roles and custom ones", async () => {
    mockFetch(routes(["rbac"]));
    const { screen } = renderApp("/admin/roles");
    await waitFor(async () => expect(await rowOf(screen, "Deployer")).toHaveTextContent("ship, never delete"));
    expect(await rowOf(screen, "Viewer")).toHaveTextContent("Built-in");
    expect(await rowOf(screen, "Operator")).toHaveTextContent("Built-in");
    expect(screen.queryByRole("status")).toBeNull();
  });

  it("creates a role from ticked cells", async () => {
    const calls = mockFetch({
      ...routes(["rbac"]),
      "POST /api/v1/ee/roles": { status: 201, json: { ...DEPLOYER, id: 6, name: "Shipper", holders: { users: [], teams: [] } } },
    });
    const { screen } = renderApp("/admin/roles");
    await userEvent.click(await screen.findByRole("button", { name: "New role" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.type(within(dialog).getByLabelText("Name"), "Shipper");
    await userEvent.click(await within(dialog).findByRole("checkbox", { name: "Containers: View" }));
    await userEvent.click(within(dialog).getByRole("checkbox", { name: "Containers: Operate" }));
    await userEvent.click(within(dialog).getByRole("button", { name: "Create" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(calls.find((c) => c.method === "POST")?.body).toEqual({
      name: "Shipper",
      description: null,
      permissions: ["containers.operate", "containers.view"],
    });
  });

  it("groups swarm rows under Docker Swarm", async () => {
    mockFetch(routes(["rbac"]));
    const { screen } = renderApp("/admin/roles");
    await userEvent.click(await screen.findByRole("button", { name: "New role" }));
    const dialog = await screen.findByRole("dialog");
    expect(await within(dialog).findByText("Docker Swarm")).toBeInTheDocument();
    expect(within(dialog).getByRole("checkbox", { name: "Configs: Reveal" })).toBeInTheDocument();
  });

  it("toggles a column across only the cells a row has", async () => {
    mockFetch(routes(["rbac"]));
    const { screen } = renderApp("/admin/roles");
    await userEvent.click(await screen.findByRole("button", { name: "New role" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.click(await within(dialog).findByRole("button", { name: "Toggle Shell for every row" }));
    expect(within(dialog).getByRole("checkbox", { name: "Containers: Shell" })).toBeChecked();
    expect(within(dialog).getByRole("checkbox", { name: "Pods: Shell" })).toBeChecked();
    expect(within(dialog).queryByRole("checkbox", { name: "Volumes: Shell" })).toBeNull();
  });

  it("warns about rows without View, Shell and creating cluster objects", async () => {
    mockFetch(routes(["rbac"]));
    const { screen } = renderApp("/admin/roles");
    await userEvent.click(await screen.findByRole("button", { name: "New role" }));
    const dialog = await screen.findByRole("dialog");
    expect(await within(dialog).findByText(/Shell can read mounted secrets/)).toBeInTheDocument();
    await userEvent.click(within(dialog).getByRole("checkbox", { name: "Volumes: Delete" }));
    expect(within(dialog).getByText("Users won't see Volumes in lists.")).toBeInTheDocument();
    await userEvent.click(within(dialog).getByRole("checkbox", { name: "Everything else: Create" }));
    expect(within(dialog).getByText(/can give a service account cluster-admin/)).toBeInTheDocument();
  });

  it("frozen: New role is disabled and an edit can only take cells away", async () => {
    mockFetch({
      ...routes([]),
      "GET /api/v1/ee/roles/5": { json: { ...DEPLOYER, holders: { users: [], teams: [] } } },
    });
    const { screen } = renderApp("/admin/roles");
    expect(await screen.findByRole("status")).toHaveTextContent("Roles are read-only until a license is installed. People keep their access.");
    expect(screen.getByRole("button", { name: "New role" })).toBeDisabled();
    await userEvent.click(within(await rowOf(screen, "Deployer")).getByRole("button", { name: "Edit" }));
    const dialog = await screen.findByRole("dialog");
    expect(await within(dialog).findByRole("checkbox", { name: "Containers: View" })).toBeEnabled();
    expect(within(dialog).getByRole("checkbox", { name: "Containers: Delete" })).toBeDisabled();
    expect(within(dialog).getByLabelText("Name")).toBeDisabled();
  });

  it("the user Access dialog offers custom roles and sends role_id", async () => {
    const calls = mockFetch({
      ...routes(["rbac", "teams"]),
      "GET /api/v1/users/2/access": { json: [] },
      "GET /api/v1/ee/users/2/access": { json: [] },
      "PUT /api/v1/users/2/access/1": {
        json: { env_id: 1, env_name: "local", env_kind: "docker-local", role: "operator", role_id: 5, role_name: "Deployer", namespaces: null },
      },
    });
    const { screen } = renderApp("/admin/users");
    const users = await screen.findByRole("table", { name: "Users" });
    await waitFor(() => expect(within(users).getAllByRole("row").length).toBeGreaterThan(2));
    const bob = within(users).getAllByRole("row").find((r) => r.textContent?.includes("bob"))!;
    await userEvent.click(within(bob).getByRole("button", { name: "Access" }));
    const dialog = await screen.findByRole("dialog");
    const role = await within(dialog).findByLabelText("Role on local");
    await waitFor(() => expect(within(role).getByRole("option", { name: "Deployer" })).toBeInTheDocument());
    await userEvent.selectOptions(role, "custom:5");
    await userEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(calls.find((c) => c.method === "PUT")?.body).toEqual({ role_id: 5 }));
  });
});
