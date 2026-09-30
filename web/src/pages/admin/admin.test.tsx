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
