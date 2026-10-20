import { waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { json, mockFetch } from "@/test/fetch";
import { ENVS, loggedIn, renderApp } from "@/test/app";

const COMPOSE = "services:\n  web:\n    image: nginx:1.27\n";

const detail = {
  stack: { id: 1, env_id: 1, name: "shop", source: "inline", deployed_revision: 2, created_at: 0, created_by: 1 },
  draft: { compose: COMPOSE, variables: ["TOKEN=••••"], updated_at: 5 },
  deployed: {
    id: 2, stack_id: 1, number: 2, compose: COMPOSE, git_commit: null, status: "deployed",
    error: null, kind: "deploy", rollback_of: null, created_at: 4, created_by: 1,
  },
};

const plan = {
  issues: [],
  warnings: [],
  plan: {
    services: [{ name: "web", action: "recreate", reasons: ["image: nginx:1.26 → nginx:1.27"], replicas_before: 1, replicas_after: 1 }],
    networks: [],
    volumes: [],
    images: [{ image: "nginx:1.27", pull: true }],
    blocking: [],
    token: "tok",
  },
  compose_before: COMPOSE.replace("1.27", "1.26"),
  compose_after: COMPOSE,
  variables_changed: [],
};

const routes = {
  ...loggedIn,
  "GET /api/v1/envs/local/stacks/shop": { json: detail },
  "POST /api/v1/envs/local/stacks/validate": { json: { issues: [], warnings: [] } },
  "PUT /api/v1/envs/local/stacks/shop/draft": { status: 204 },
  "POST /api/v1/envs/local/stacks/shop/plan": { json: plan },
  "GET /api/v1/envs/local/stacks/shop/revisions": {
    json: [detail.deployed, { ...detail.deployed, id: 1, number: 1, status: "superseded" }],
  },
};

const withPerms = (permissions: string[]) => ({
  "GET /api/v1/envs": {
    json: ENVS.map((e) => ({ ...e, access: { role: "operator", namespaces: null, grants: [{ namespaces: null, permissions }] } })),
  },
});

describe("stack page", () => {
  it("shows validation problems with their paths", async () => {
    mockFetch({
      ...routes,
      "POST /api/v1/envs/local/stacks/validate": {
        json: { issues: [{ path: "services.web.build", message: "'build' is not supported" }], warnings: [] },
      },
    });
    const { screen } = renderApp("/envs/local/stacks/shop");
    const problems = await screen.findByRole("list", { name: "Problems" }, { timeout: 3000 });
    expect(problems).toHaveTextContent("services.web.build: 'build' is not supported");
  });

  it("saves the draft, then shows recreate reasons and the diff", async () => {
    const calls = mockFetch(routes);
    const { screen } = renderApp("/envs/local/stacks/shop");
    await userEvent.click(await screen.findByRole("button", { name: "Review deploy" }));
    const dialog = await screen.findByRole("dialog", { name: "Deploy shop" });
    expect(await within(dialog).findByText("image: nginx:1.26 → nginx:1.27")).toBeInTheDocument();
    expect(within(dialog).getByLabelText("Compose file changes")).toHaveTextContent("- image: nginx:1.26+ image: nginx:1.27");
    expect(calls.some((c) => c.method === "PUT" && c.path.endsWith("/shop/draft"))).toBe(true);
    expect(within(dialog).getByRole("button", { name: "Deploy" })).toBeEnabled();
  });

  it("disables Deploy when there are problems or blockers", async () => {
    mockFetch({
      ...routes,
      "POST /api/v1/envs/local/stacks/shop/plan": { json: { ...plan, plan: { ...plan.plan, blocking: ["network shop_default changed"] } } },
    });
    const { screen } = renderApp("/envs/local/stacks/shop");
    await userEvent.click(await screen.findByRole("button", { name: "Review deploy" }));
    const dialog = await screen.findByRole("dialog", { name: "Deploy shop" });
    expect(await within(dialog).findByText("network shop_default changed")).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "Deploy" })).toBeDisabled();
  });

  it("offers to review again when the plan went stale", async () => {
    mockFetch({
      ...routes,
      "POST /api/v1/envs/local/stacks/shop/deploy": () =>
        json(409, { code: "conflict", message: "the stack changed since this plan was made; review again" }),
    });
    const { screen } = renderApp("/envs/local/stacks/shop");
    await userEvent.click(await screen.findByRole("button", { name: "Review deploy" }));
    const dialog = await screen.findByRole("dialog", { name: "Deploy shop" });
    await userEvent.click(await within(dialog).findByRole("button", { name: "Deploy" }));
    expect(await within(dialog).findByRole("button", { name: "Review again" })).toBeInTheDocument();
    expect(within(dialog).getByText(/review again$/)).toBeInTheDocument();
  });

  it("gives an operate-only user a read-only editor that plans the saved draft", async () => {
    const calls = mockFetch({ ...routes, ...withPerms(["stacks.view", "stacks.operate"]) });
    const { screen } = renderApp("/envs/local/stacks/shop");
    await userEvent.click(await screen.findByRole("button", { name: "Review deploy" }));
    await screen.findByRole("dialog", { name: "Deploy shop" });
    expect(screen.queryByRole("button", { name: "Save draft" })).not.toBeInTheDocument();
    expect(screen.getAllByLabelText("Compose file")[0].tagName).toBe("PRE");
    expect(calls.some((c) => c.method === "PUT")).toBe(false);
  });

  it("rolls back through a review of that revision", async () => {
    const calls = mockFetch(routes);
    const { screen } = renderApp("/envs/local/stacks/shop");
    await userEvent.click(await screen.findByRole("tab", { name: "Revisions" }));
    const table = await screen.findByRole("table", { name: "Revisions" });
    const buttons = await within(table).findAllByRole("button", { name: "Roll back to this" });
    expect(buttons).toHaveLength(1);
    await userEvent.click(buttons[0]);
    await screen.findByRole("dialog", { name: "Roll back shop to revision 1" });
    await waitFor(() =>
      expect(calls.find((c) => c.path.endsWith("/shop/plan"))?.body).toEqual({ source: "revision", revision: 1 }),
    );
  });

  it("needs the stack name typed to remove its volumes", async () => {
    const calls = mockFetch({
      ...routes,
      "GET /api/v1/envs/local/volumes": {
        json: [{ name: "shop_data", driver: "local", mountpoint: "", created_at: null, labels: { "com.docker.compose.project": "shop" }, in_use: false }],
      },
      "DELETE /api/v1/envs/local/stacks/shop": { status: 204 },
      "GET /api/v1/envs/local/stacks": { json: [] },
    });
    const { screen } = renderApp("/envs/local/stacks/shop");
    await userEvent.click(await screen.findByRole("button", { name: "Remove" }));
    const dialog = await screen.findByRole("dialog", { name: "Remove shop?" });
    await userEvent.click(within(dialog).getByRole("checkbox"));
    expect(await within(dialog).findByText("shop_data")).toBeInTheDocument();
    await userEvent.click(within(dialog).getByRole("button", { name: "Remove" }));
    expect(await within(dialog).findByText("Type shop to remove its volumes")).toBeInTheDocument();
    expect(calls.some((c) => c.method === "DELETE")).toBe(false);
    await userEvent.type(within(dialog).getByLabelText("Type shop to confirm"), "shop");
    await userEvent.click(within(dialog).getByRole("button", { name: "Remove" }));
    await waitFor(() => expect(calls.find((c) => c.method === "DELETE")?.path).toBe("/api/v1/envs/local/stacks/shop?volumes=true"));
  });

  it("shows masked variables as the server sends them", async () => {
    mockFetch({ ...routes, ...withPerms(["stacks.view", "stacks.create", "stacks.operate"]) });
    const { screen } = renderApp("/envs/local/stacks/shop");
    expect(await screen.findByLabelText("Variable 1 name")).toHaveValue("TOKEN");
    expect(screen.getByLabelText("Variable 1 value")).toHaveValue("••••");
  });
});
