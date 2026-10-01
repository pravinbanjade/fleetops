import { waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { User } from "@/api/gen/types.gen";
import { json, mockFetch } from "@/test/fetch";
import { ADMIN, ENVS, loggedIn, renderApp } from "@/test/app";

const BOB: User = { id: 2, username: "bob", role: "user", created_at: 1_790_000_000, updated_at: 0 };
const routes = { ...loggedIn, "GET /api/v1/users": { json: [ADMIN.user, BOB] } };

const row = (screen: typeof import("@testing-library/react").screen, name: string) =>
  within(screen.getByRole("table", { name: /Users|Environments/ }))
    .getAllByRole("row")
    .find((r) => r.textContent?.includes(name))!;

describe("admin access", () => {
  it("shows the administration links to admins only", async () => {
    mockFetch(routes);
    const { screen } = renderApp("/envs/local");
    const nav = await screen.findByRole("navigation", { name: "Administration" });
    expect(within(nav).getByRole("link", { name: "Users" })).toHaveAttribute("href", "/admin/users");
  });

  it("sends a user who is not an admin away", async () => {
    mockFetch({ ...routes, "GET /api/v1/auth/me": { json: { ...ADMIN, user: BOB } } });
    const { screen, history } = renderApp("/admin/users");
    await waitFor(() => expect(history.location.pathname).toBe("/envs/local"));
    expect(screen.queryByRole("navigation", { name: "Administration" })).not.toBeInTheDocument();
  });
});

describe("users", () => {
  it("lists users and marks the current one", async () => {
    mockFetch(routes);
    const { screen } = renderApp("/admin/users");
    await waitFor(() => expect(row(screen, "admin")).toHaveTextContent("(you)"));
    expect(row(screen, "bob")).toHaveTextContent("user");
  });

  it("adds a user", async () => {
    let users = [ADMIN.user, BOB];
    const calls = mockFetch({
      ...routes,
      "GET /api/v1/users": () => json(200, users),
      "POST /api/v1/users": (c) => {
        const u = { ...BOB, id: 3, ...(c.body as object) } as User;
        users = [...users, u];
        return json(201, u);
      },
    });
    const { screen } = renderApp("/admin/users");
    await userEvent.click(await screen.findByRole("button", { name: "Add user" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.type(within(dialog).getByLabelText("Username"), "carol");
    await userEvent.type(within(dialog).getByLabelText("Password"), "a-long-password");
    await userEvent.selectOptions(within(dialog).getByLabelText("Role"), "admin");
    await userEvent.click(within(dialog).getByRole("button", { name: "Add" }));
    await waitFor(() => expect(row(screen, "carol")).toBeTruthy());
    expect(calls.find((c) => c.method === "POST")?.body).toEqual({
      username: "carol",
      password: "a-long-password",
      role: "admin",
    });
  });

  it("shows the server's reason a user cannot be added", async () => {
    mockFetch({
      ...routes,
      "POST /api/v1/users": () => json(400, { code: "invalid", message: "password must be 12-128 characters" }),
    });
    const { screen } = renderApp("/admin/users");
    await userEvent.click(await screen.findByRole("button", { name: "Add user" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.type(within(dialog).getByLabelText("Username"), "carol");
    await userEvent.type(within(dialog).getByLabelText("Password"), "short");
    await userEvent.click(within(dialog).getByRole("button", { name: "Add" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("password must be 12-128 characters");
  });

  it("changes a role and resets a password", async () => {
    const calls = mockFetch({ ...routes, "PATCH /api/v1/users/2": { json: BOB } });
    const { screen } = renderApp("/admin/users");
    await waitFor(() => expect(row(screen, "bob")).toBeTruthy());
    await userEvent.click(within(row(screen, "bob")).getByRole("button", { name: "Change role" }));
    let dialog = await screen.findByRole("dialog");
    await userEvent.selectOptions(within(dialog).getByLabelText("Role"), "admin");
    await userEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    await userEvent.click(within(row(screen, "bob")).getByRole("button", { name: "Reset password" }));
    dialog = await screen.findByRole("dialog");
    await userEvent.type(within(dialog).getByLabelText("New password"), "another-long-one");
    await userEvent.click(within(dialog).getByRole("button", { name: "Reset password" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(calls.filter((c) => c.method === "PATCH").map((c) => c.body)).toEqual([
      { role: "admin" },
      { password: "another-long-one" },
    ]);
  });

  it("refuses to delete the last admin with the server's reason", async () => {
    mockFetch({
      ...routes,
      "DELETE /api/v1/users/1": () => json(409, { code: "conflict", message: "cannot delete the last admin" }),
    });
    const { screen } = renderApp("/admin/users");
    await waitFor(() => expect(row(screen, "admin")).toBeTruthy());
    await userEvent.click(within(row(screen, "admin")).getByRole("button", { name: "Delete" }));
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent("This is your own account");
    await userEvent.click(within(dialog).getByRole("button", { name: "Delete" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("cannot delete the last admin");
  });
});

describe("environments", () => {
  it("lists environments with their status", async () => {
    mockFetch(routes);
    const { screen } = renderApp("/admin/envs");
    await waitFor(() => expect(row(screen, "local")).toHaveTextContent("up"));
    expect(row(screen, "local")).toHaveTextContent("unix:///var/run/docker.sock");
  });

  it("adds an environment", async () => {
    const calls = mockFetch({
      ...routes,
      "POST /api/v1/envs": { status: 201, json: { ...ENVS[0], id: 2, name: "remote", url: "tcp://10.0.0.5:2375" } },
    });
    const { screen } = renderApp("/admin/envs");
    await userEvent.click(await screen.findByRole("button", { name: "Add environment" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.type(within(dialog).getByLabelText("Name"), "remote");
    await userEvent.clear(within(dialog).getByLabelText("Docker URL"));
    await userEvent.type(within(dialog).getByLabelText("Docker URL"), "tcp://10.0.0.5:2375");
    await userEvent.click(within(dialog).getByRole("button", { name: "Add" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(calls.find((c) => c.method === "POST")?.body).toEqual({
      name: "remote",
      url: "tcp://10.0.0.5:2375",
      kind: "docker-local",
    });
  });

  it("renames an environment, sending only what changed", async () => {
    const calls = mockFetch({ ...routes, "PATCH /api/v1/envs/local": { json: { ...ENVS[0], name: "laptop" } } });
    const { screen } = renderApp("/admin/envs");
    await waitFor(() => expect(row(screen, "local")).toBeTruthy());
    await userEvent.click(within(row(screen, "local")).getByRole("button", { name: "Edit" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.clear(within(dialog).getByLabelText("Name"));
    await userEvent.type(within(dialog).getByLabelText("Name"), "laptop");
    await userEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(calls.find((c) => c.method === "PATCH")?.body).toEqual({ name: "laptop", url: null });
  });

  it("deletes an environment after confirming", async () => {
    const calls = mockFetch({ ...routes, "DELETE /api/v1/envs/local": { status: 204 } });
    const { screen } = renderApp("/admin/envs");
    await waitFor(() => expect(row(screen, "local")).toBeTruthy());
    await userEvent.click(within(row(screen, "local")).getByRole("button", { name: "Delete" }));
    await userEvent.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(calls.some((c) => c.method === "DELETE")).toBe(true));
  });
});

describe("agent environments", () => {
  const AGENT = {
    id: 2,
    name: "web-01",
    kind: "docker-agent",
    url: "agent://2",
    created_at: 0,
    updated_at: 0,
    agent: {
      connected: false,
      enrolled: true,
      last_seen_at: Math.floor(Date.now() / 1000) - 600,
      agent_version: "0.2.0",
      host: { hostname: "web-01" },
      last_error: null,
      join_expires_at: null,
    },
  };
  const LOCAL = { ...ENVS[0], agent: null };
  const enrollment = (token: string) => ({
    join_token: token,
    expires_at: Math.floor(Date.now() / 1000) + 900,
    command: `docker run … --join-token ${token}`,
  });
  const agentRoutes = {
    ...routes,
    "GET /api/v1/envs": { json: [LOCAL, AGENT] },
    "GET /api/v1/envs/web-01/status": { json: { state: "down", reason: "agent offline: connection lost" } },
  };

  it("adds an agent environment and shows its one-time command", async () => {
    const calls = mockFetch({
      ...agentRoutes,
      "POST /api/v1/envs": { status: 201, json: { ...AGENT, agent: undefined, enrollment: enrollment("fj_abc") } },
    });
    const { screen } = renderApp("/admin/envs");
    await userEvent.click(await screen.findByRole("button", { name: "Add environment" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.click(within(dialog).getByRole("radio", { name: /agent/i }));
    expect(within(dialog).queryByLabelText("Docker URL")).not.toBeInTheDocument();
    await userEvent.type(within(dialog).getByLabelText("Name"), "web-01");
    await userEvent.click(within(dialog).getByRole("button", { name: "Add" }));
    expect(calls.find((c) => c.method === "POST")?.body).toEqual({ name: "web-01", kind: "docker-agent" });
    const shown = await screen.findByRole("dialog", { name: /run this on the remote host/i });
    expect(within(shown).getByText(/--join-token fj_abc/)).toBeInTheDocument();
    expect(within(shown).getByRole("button", { name: /copy/i })).toBeInTheDocument();
    expect(within(shown).getByText(/expires in 1[45] min/i)).toBeInTheDocument();
    await userEvent.click(within(shown).getByRole("button", { name: "Done" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
  });

  it("shows agent state and offers rotate only for agents", async () => {
    mockFetch(agentRoutes);
    const { screen } = renderApp("/admin/envs");
    await waitFor(() => expect(row(screen, "web-01")).toBeTruthy());
    expect(row(screen, "web-01")).toHaveTextContent("0.2.0");
    expect(row(screen, "web-01")).toHaveTextContent("last seen 10 minutes ago");
    expect(within(row(screen, "web-01")).getByRole("button", { name: "Rotate key" })).toBeInTheDocument();
    expect(within(row(screen, "local")).queryByRole("button", { name: "Rotate key" })).not.toBeInTheDocument();
  });

  it("rotating asks first, then shows the new command", async () => {
    const calls = mockFetch({ ...agentRoutes, "POST /api/v1/envs/web-01/agent/rotate": { json: enrollment("fj_new") } });
    const { screen } = renderApp("/admin/envs");
    await waitFor(() => expect(row(screen, "web-01")).toBeTruthy());
    await userEvent.click(within(row(screen, "web-01")).getByRole("button", { name: "Rotate key" }));
    const confirm = await screen.findByRole("dialog");
    expect(confirm).toHaveTextContent(/disconnects the running agent/i);
    await userEvent.click(within(confirm).getByRole("button", { name: "Rotate" }));
    expect(await screen.findByText(/--join-token fj_new/)).toBeInTheDocument();
    expect(calls.some((c) => c.method === "POST" && c.path.endsWith("/agent/rotate"))).toBe(true);
  });

  it("an agent's URL is not editable", async () => {
    mockFetch(agentRoutes);
    const { screen } = renderApp("/admin/envs");
    await waitFor(() => expect(row(screen, "web-01")).toBeTruthy());
    await userEvent.click(within(row(screen, "web-01")).getByRole("button", { name: "Edit" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).queryByLabelText("Docker URL")).not.toBeInTheDocument();
  });
});
