import { waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { User } from "@/api/gen/types.gen";
import { mockFetch } from "@/test/fetch";
import { ADMIN, loggedIn, renderApp } from "@/test/app";
import type { TeamDetail, TeamSummary } from "./teams";

const BOB: User = { id: 2, username: "bob", role: "user", created_at: 0, updated_at: 0 };
const CAROL: User = { id: 3, username: "carol", role: "user", created_at: 0, updated_at: 0 };
const OPS: TeamSummary = { id: 7, name: "ops", description: "on call", members: 1, environments: 1 };
const OPS_DETAIL: TeamDetail = {
  id: 7,
  name: "ops",
  description: "on call",
  members: [BOB],
  synced: [],
  access: [{ env_id: 1, env_name: "local", env_kind: "docker-local", role: "viewer", namespaces: null }],
};

const routes = (features: string[]) => ({
  ...loggedIn,
  "GET /api/v1/edition": { json: { edition: "enterprise", features } },
  "GET /api/v1/users": { json: [ADMIN.user, BOB, CAROL] },
  "GET /api/v1/ee/teams": { json: [OPS] },
  "GET /api/v1/ee/teams/7": { json: OPS_DETAIL },
});

const row = (screen: typeof import("@testing-library/react").screen, name: string) =>
  within(screen.getByRole("table", { name: "Teams" }))
    .getAllByRole("row")
    .find((r) => r.textContent?.includes(name))!;

describe("teams", () => {
  it("lists teams and creates one", async () => {
    const calls = mockFetch({
      ...routes(["teams"]),
      "POST /api/v1/ee/teams": { status: 201, json: { ...OPS_DETAIL, id: 8, name: "dev", members: [], synced: [], access: [] } },
    });
    const { screen } = renderApp("/admin/teams");
    await waitFor(() => expect(row(screen, "ops")).toHaveTextContent("on call"));
    expect(screen.queryByRole("status")).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: "New team" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.type(within(dialog).getByLabelText("Name"), "dev");
    await userEvent.click(within(dialog).getByRole("button", { name: "Create" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(calls.find((c) => c.method === "POST")?.body).toEqual({ name: "dev", description: null });
  });

  it("adds and removes members", async () => {
    const calls = mockFetch({
      ...routes(["teams"]),
      "PUT /api/v1/ee/teams/7/members/3": { status: 204 },
      "DELETE /api/v1/ee/teams/7/members/2": { status: 204 },
    });
    const { screen } = renderApp("/admin/teams");
    await waitFor(() => expect(row(screen, "ops")).toBeTruthy());
    await userEvent.click(within(row(screen, "ops")).getByRole("button", { name: "Members" }));
    const dialog = await screen.findByRole("dialog");
    await within(dialog).findByText("bob");
    await userEvent.selectOptions(within(dialog).getByLabelText("User to add"), "carol");
    await userEvent.click(within(dialog).getByRole("button", { name: "Add" }));
    await userEvent.click(within(dialog).getByRole("button", { name: "Remove bob" }));
    await waitFor(() =>
      expect(calls.filter((c) => c.method !== "GET").map((c) => `${c.method} ${c.path}`)).toEqual([
        "PUT /api/v1/ee/teams/7/members/3",
        "DELETE /api/v1/ee/teams/7/members/2",
      ]),
    );
  });

  it("frozen shows the banner, disables New team and Add member, keeps Remove enabled", async () => {
    mockFetch(routes([]));
    const { screen } = renderApp("/admin/teams");
    expect(await screen.findByRole("status")).toHaveTextContent(
      "Teams are read-only until a license is installed. Members keep their access.",
    );
    expect(screen.getByRole("link", { name: "Manage license" })).toHaveAttribute("href", "/admin/license");
    expect(screen.getByRole("button", { name: "New team" })).toBeDisabled();
    await waitFor(() => expect(row(screen, "ops")).toBeTruthy());
    expect(within(row(screen, "ops")).getByRole("button", { name: "Rename" })).toBeDisabled();
    await userEvent.click(within(row(screen, "ops")).getByRole("button", { name: "Members" }));
    const dialog = await screen.findByRole("dialog");
    await within(dialog).findByText("bob");
    expect(within(dialog).getByRole("button", { name: "Add" })).toBeDisabled();
    expect(within(dialog).getByRole("button", { name: "Remove bob" })).toBeEnabled();
  });

  it("frozen access editor cannot raise viewer to operator", async () => {
    mockFetch(routes([]));
    const { screen } = renderApp("/admin/teams");
    await waitFor(() => expect(row(screen, "ops")).toBeTruthy());
    await userEvent.click(within(row(screen, "ops")).getByRole("button", { name: "Access" }));
    const dialog = await screen.findByRole("dialog");
    const role = await within(dialog).findByLabelText("Role on local");
    await waitFor(() => expect(role).toHaveValue("viewer"));
    expect(within(role).getByRole("option", { name: "Operator" })).toBeDisabled();
    expect(within(role).getByRole("option", { name: "None" })).toBeEnabled();
    expect(within(dialog).getByText(/Operator on one namespace stays Operator on that namespace only/)).toBeInTheDocument();
    expect(within(dialog).queryByText(/makes them Operator on the whole cluster/)).toBeNull();
  });

  it("user access dialog shows team-derived access with team names", async () => {
    mockFetch({
      ...routes(["teams"]),
      "GET /api/v1/users/2/access": { json: [] },
      "GET /api/v1/ee/users/2/access": {
        json: [
          {
            env_id: 1,
            env_name: "local",
            env_kind: "docker-local",
            role: "operator",
            namespaces: null,
            sources: [
              { team_id: 7, team_name: "ops", role: "operator", namespaces: null },
              { team_id: 8, team_name: "dev", role: "viewer", namespaces: null },
            ],
          },
        ],
      },
    });
    const { screen } = renderApp("/admin/users");
    const users = await screen.findByRole("table", { name: "Users" });
    await waitFor(() => expect(within(users).getAllByRole("row").length).toBeGreaterThan(2));
    const bob = within(users).getAllByRole("row").find((r) => r.textContent?.includes("bob"))!;
    await userEvent.click(within(bob).getByRole("button", { name: "Access" }));
    const dialog = await screen.findByRole("dialog");
    const teams = await within(dialog).findByRole("region", { name: "Through teams" });
    expect(teams).toHaveTextContent("local");
    expect(teams).toHaveTextContent("operator");
    expect(teams).toHaveTextContent("ops, dev");
  });

  it("an environment's access view includes team members", async () => {
    mockFetch({
      ...routes(["teams"]),
      "GET /api/v1/envs/local/access": { json: [] },
      "GET /api/v1/ee/envs/local/teams": {
        json: [{ team_id: 7, team_name: "ops", role: "operator", namespaces: null, members: ["bob", "carol"] }],
      },
    });
    const { screen } = renderApp("/admin/envs");
    const envs = await screen.findByRole("table", { name: "Environments" });
    await waitFor(() => expect(within(envs).getAllByRole("row").length).toBeGreaterThan(1));
    const local = within(envs).getAllByRole("row").find((r) => r.textContent?.includes("local"))!;
    await userEvent.click(within(local).getByRole("button", { name: "Access" }));
    const dialog = await screen.findByRole("dialog");
    const teams = await within(dialog).findByRole("region", { name: "Through teams" });
    expect(teams).toHaveTextContent("ops");
    expect(teams).toHaveTextContent("operator");
    expect(teams).toHaveTextContent("bob, carol");
    expect(within(dialog).queryByText("Only admins can use this environment.")).toBeNull();
  });
});
