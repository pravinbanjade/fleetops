import { waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { mockFetch } from "@/test/fetch";
import { ENVS, loggedIn, renderApp } from "@/test/app";

/** Bob: an operator limited to stack `blog` on the swarm `prod`. */
const SWARM = {
  id: 7,
  name: "prod",
  kind: "swarm-agent",
  url: "agent://7",
  created_at: 0,
  updated_at: 0,
  agent: { connected: true, enrolled: true, custom_resources: false },
  swarm_nodes_connected: 2,
  access: { role: "operator", namespaces: ["blog"], grants: [{ namespaces: ["blog"], permissions: [
    "swarm-services.view", "swarm-services.operate", "swarm-services.create", "swarm-services.delete",
    "swarm-secrets.view", "swarm-secrets.create", "swarm-nodes.view", "containers.view", "images.view",
    "volumes.view", "networks.view", "stacks.view", "stacks.operate",
  ] }] },
};

const svc = (id: string, name: string, stack: string | null) => ({
  id, name, stack, mode: "replicated", replicas_desired: 1, tasks_running: 1, tasks_desired: 1,
  image: "nginx:1.27", ports: [], update_state: null, updated_at: 0, labels: {},
});

const routes = {
  ...loggedIn,
  "GET /api/v1/auth/me": { json: { csrf_token: "c", user: { id: 2, username: "bob", role: "user", created_at: 0, updated_at: 0 } } },
  "GET /api/v1/envs": { json: [...ENVS, SWARM] },
  "GET /api/v1/envs/prod/status": {
    json: { state: "up", version: { kind: "swarm", engine: "27.3.1", cluster_id: "c1", managers: 1, workers: 1, serving_node: "n1" } },
  },
  "GET /api/v1/envs/prod/swarm/services": { json: [svc("s1", "blog_web", "blog"), svc("s2", "agent", null)] },
  "GET /api/v1/envs/prod@n2/status": {
    json: { state: "up", version: { kind: "docker", version: "27.3.1", api_version: "1.47", os: "linux", arch: "amd64" } },
  },
  "GET /api/v1/envs/prod@n2/containers": { json: [] },
};

describe("a grant limited to stacks", () => {
  it("acts on its stacks' services only, and creates nothing outside a stack", async () => {
    mockFetch(routes);
    const { screen } = renderApp("/envs/prod/swarm/services");
    const blog = await screen.findByRole("row", { name: /blog_web/ });
    await userEvent.click(within(blog).getByRole("button", { name: /blog_web/ }));
    expect(await screen.findByRole("menuitem", { name: "Scale" })).toBeInTheDocument();
    await userEvent.keyboard("{Escape}");
    const agent = screen.getByRole("row", { name: /agent/ });
    expect(within(agent).queryByRole("button", { name: /agent/ })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Create service" })).not.toBeInTheDocument();
  });

  it("sees only containers on a node's pages", async () => {
    mockFetch(routes);
    const { screen } = renderApp("/envs/prod@n2/containers");
    const nav = await screen.findByRole("navigation", { name: "Environment" });
    await waitFor(() => expect(within(nav).getByRole("link", { name: /Containers/ })).toBeInTheDocument());
    for (const name of [/Images/, /Volumes/, /Networks/]) expect(within(nav).queryByRole("link", { name })).not.toBeInTheDocument();
  });
});
