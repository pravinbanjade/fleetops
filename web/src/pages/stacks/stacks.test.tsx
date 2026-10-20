import { within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { mockFetch } from "@/test/fetch";
import { ENVS, loggedIn, renderApp } from "@/test/app";

const stack = {
  name: "shop",
  source: "inline",
  deployed_revision: 2,
  last_status: "deployed",
  updated_at: 0,
  draft_differs: true,
};

const routes = {
  ...loggedIn,
  "GET /api/v1/envs/local/stacks": { json: [stack] },
  "GET /api/v1/envs/local/compose-projects": {
    json: [
      { name: "shop", status: "running", containers: 1, running: 1, services: ["web"], managed: true },
      { name: "blog", status: "running", containers: 1, running: 1, services: ["web"], managed: false },
    ],
  },
};

const withPerms = (permissions: string[]) => ({
  "GET /api/v1/envs": {
    json: ENVS.map((e) => ({ ...e, access: { role: "operator", namespaces: null, grants: [{ namespaces: null, permissions }] } })),
  },
});

describe("stacks list", () => {
  it("lists stacks and unmanaged projects", async () => {
    mockFetch(routes);
    const { screen } = renderApp("/envs/local/stacks");
    const table = await screen.findByRole("table", { name: "Stacks" });
    expect(await within(table).findByRole("link", { name: "shop" })).toBeInTheDocument();
    expect(within(table).getByText("#2")).toBeInTheDocument();
    expect(within(table).getByText("Update pending")).toBeInTheDocument();
    expect(await screen.findByRole("heading", { name: "Other Compose projects" })).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "blog" })).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "shop" })).not.toBeInTheDocument();
  });

  it("hides New stack without create", async () => {
    mockFetch({ ...routes, ...withPerms(["stacks.view"]) });
    const { screen } = renderApp("/envs/local/stacks");
    await screen.findByRole("table", { name: "Stacks" });
    expect(screen.queryByRole("button", { name: "New stack" })).not.toBeInTheDocument();
  });

  it("validates the new stack's name before creating it", async () => {
    const calls = mockFetch({ ...routes, "POST /api/v1/envs/local/stacks": { status: 201, json: stack } });
    const { screen } = renderApp("/envs/local/stacks");
    await userEvent.click(await screen.findByRole("button", { name: "New stack" }));
    const dialog = await screen.findByRole("dialog", { name: "New stack" });
    const input = within(dialog).getByLabelText("Name");
    await userEvent.type(input, "Bad Name");
    expect(within(dialog).getByText(/Use lowercase letters/)).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "Create" })).toBeDisabled();
    await userEvent.clear(input);
    await userEvent.type(input, "shop");
    await userEvent.click(within(dialog).getByRole("button", { name: "Create" }));
    const post = calls.find((c) => c.method === "POST" && c.path === "/api/v1/envs/local/stacks");
    expect(post?.body).toMatchObject({ name: "shop" });
  });

  it("redirects the old compose page", async () => {
    mockFetch(routes);
    const { history, screen } = renderApp("/envs/local/compose");
    await screen.findByRole("table", { name: "Stacks" });
    expect(history.location.pathname).toBe("/envs/local/stacks");
  });
});
