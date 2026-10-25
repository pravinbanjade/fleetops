import { waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { mockFetch } from "@/test/fetch";
import { ENVS, loggedIn, renderApp } from "@/test/app";

const SWARM = {
  id: 7,
  name: "prod",
  kind: "swarm-agent",
  url: "agent://7",
  created_at: 0,
  updated_at: 0,
  agent: { connected: true, enrolled: true, custom_resources: false },
  swarm_nodes_connected: 2,
};

const COMPOSE = "services:\n  web:\n    image: nginx:1.27\n";

const svc = (id: string, name: string, stack: string | null, labels: Record<string, string> = {}) => ({
  id,
  name,
  stack,
  mode: "replicated",
  replicas_desired: 3,
  tasks_running: 2,
  tasks_desired: 3,
  image: "nginx:1.27",
  ports: [],
  update_state: null,
  updated_at: 0,
  labels,
});

const deployed = {
  id: 1, stack_id: 1, number: 1, compose: COMPOSE, git_commit: null, status: "deployed",
  error: null, kind: "deploy", rollback_of: null, created_at: 4, created_by: 1,
};

const plan = {
  issues: [],
  warnings: [],
  plan: {
    services: [
      { name: "web", action: "update", reasons: ["image: nginx:1.26 → nginx:1.27"], replicas_before: 3, replicas_after: 3, drift: ["replicas: 3 → 7"] },
    ],
    networks: [],
    volumes: [],
    images: [],
    secrets: [
      { name: "shop_db_abc", action: "create" },
      { name: "shop_db_old", action: "remove" },
    ],
    configs: [],
    blocking: [],
    token: "tok",
  },
  compose_before: COMPOSE,
  compose_after: COMPOSE,
  variables_changed: [],
};

const routes = {
  ...loggedIn,
  "GET /api/v1/envs": { json: [...ENVS, SWARM] },
  "GET /api/v1/envs/prod/status": {
    json: { state: "up", version: { kind: "swarm", engine: "27.3.1", cluster_id: "c1", managers: 1, workers: 2, serving_node: "n1" } },
  },
  "GET /api/v1/envs/prod/stacks": {
    json: [
      { name: "shop", source: "inline", deployed_revision: 1, last_status: "deployed", updated_at: 1, draft_differs: false, update_available: false, paused: false, sync_error: null },
    ],
  },
  "GET /api/v1/envs/prod/swarm/services": {
    json: [svc("s1", "shop_web", "shop"), svc("s9", "ext_api", "ext"), svc("s8", "loose", null)],
  },
  "GET /api/v1/envs/prod/stacks/shop": {
    json: {
      stack: { id: 1, env_id: 7, name: "shop", source: "inline", deployed_revision: 1, created_at: 0, created_by: 1 },
      draft: { compose: COMPOSE, variables: [], updated_at: 5 },
      deployed,
    },
  },
  "GET /api/v1/envs/prod/stacks/shop/services": {
    json: [{ key: "web", service: svc("s1", "shop_web", "shop"), managed: true, drift: ["replicas: 3 → 7"] }],
  },
  "GET /api/v1/envs/prod/stacks/shop/revisions": { json: [deployed] },
  "POST /api/v1/envs/prod/stacks/validate": { json: { issues: [], warnings: [] } },
  "PUT /api/v1/envs/prod/stacks/shop/draft": { status: 204 },
  "POST /api/v1/envs/prod/stacks/shop/plan": { json: plan },
  "GET /api/v1/envs/prod/stacks/shop/files": { json: [] },
};

describe("Swarm stacks", () => {
  it("swarms list their stacks, then the stacks deployed outside FleetOps", async () => {
    mockFetch(routes);
    const { screen, history } = renderApp("/envs/prod/stacks");
    expect(await screen.findByRole("link", { name: "shop" })).toHaveAttribute("href", "/envs/prod/stacks/shop");
    const other = await screen.findByRole("region", { name: "Other stacks" });
    expect(within(other).getByText("ext")).toBeInTheDocument();
    expect(within(other).getByRole("link", { name: "ext_api" })).toHaveAttribute("href", "/envs/prod/swarm/services/s9");
    expect(within(other).queryByText("shop")).not.toBeInTheDocument();
    expect(screen.queryByText("Other Compose projects")).not.toBeInTheDocument();
    expect(history.location.pathname).toBe("/envs/prod/stacks");
    const nav = screen.getByRole("navigation", { name: "Environment" });
    expect(within(nav).getByRole("link", { name: /Stacks/ })).toHaveAttribute("href", "/envs/prod/stacks");
  });

  it("a Swarm stack shows its services with drift, and no container actions", async () => {
    mockFetch(routes);
    const { screen } = renderApp("/envs/prod/stacks/shop");
    await screen.findByRole("tab", { name: "Services" });
    for (const name of ["Start", "Stop", "Restart"]) expect(screen.queryByRole("button", { name })).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("tab", { name: "Services" }));
    const row = await screen.findByRole("row", { name: /web/ });
    expect(row).toHaveTextContent("2/3 running");
    expect(within(row).getByText("Drift")).toHaveAccessibleDescription(/replicas: 3 → 7/);
    expect(within(row).getByRole("link", { name: "web" })).toHaveAttribute("href", "/envs/prod/swarm/services/s1");
  });

  it("the review shows updates, drift, and secrets to create and remove", async () => {
    mockFetch(routes);
    const { screen } = renderApp("/envs/prod/stacks/shop");
    await userEvent.click(await screen.findByRole("button", { name: "Review deploy" }));
    const dialog = await screen.findByRole("dialog", { name: "Deploy shop" });
    expect(
      within(dialog).getByText(
        "Swarm rolls each service out with its update policy. If the deploy fails, what was applied stays; roll back to an earlier revision to undo it.",
      ),
    ).toBeInTheDocument();
    expect(await within(dialog).findByRole("heading", { name: "Update" })).toBeInTheDocument();
    expect(within(dialog).getByText("image: nginx:1.26 → nginx:1.27")).toBeInTheDocument();
    expect(within(dialog).getByText("Changed outside the stack: replicas: 3 → 7")).toBeInTheDocument();
    const objects = within(dialog).getByRole("list", { name: "Networks and volumes" });
    expect(objects).toHaveTextContent("Create secret shop_db_abc");
    expect(objects).toHaveTextContent("Remove secret shop_db_old");
  });

  it("removing a Swarm stack offers no volumes", async () => {
    mockFetch(routes);
    const { screen } = renderApp("/envs/prod/stacks/shop");
    await userEvent.click(await screen.findByRole("button", { name: "Remove" }));
    const dialog = await screen.findByRole("dialog", { name: "Remove shop?" });
    expect(
      within(dialog).getByText(
        "Removes the stack's services, networks, secrets and configs, and its revisions in FleetOps. Volumes stay on the nodes.",
      ),
    ).toBeInTheDocument();
    expect(within(dialog).queryByText("Also remove volumes")).not.toBeInTheDocument();
  });

  it("a service a stack manages says so", async () => {
    mockFetch({
      ...routes,
      "GET /api/v1/envs/prod/swarm/services/s1": {
        json: {
          ...svc("s1", "shop_web", "shop", { "io.fleetops.stack.spec-hash": "abc", "com.docker.stack.namespace": "shop" }),
          spec: {},
          update_status: null,
          version: 3,
        },
      },
      "GET /api/v1/envs/prod/swarm/tasks": { json: [] },
    });
    const { screen } = renderApp("/envs/prod/swarm/services/s1");
    const banner = await screen.findByText(/Managed by stack/);
    expect(banner).toHaveTextContent("Managed by stack shop; changes here are overwritten on the next deploy.");
    await waitFor(() => expect(within(banner).getByRole("link", { name: "shop" })).toHaveAttribute("href", "/envs/prod/stacks/shop"));
  });
});
