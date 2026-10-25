import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { mockFetch } from "@/test/fetch";
import { ENVS, loggedIn, renderApp } from "@/test/app";

const SWARM = { id: 7, name: "prod", kind: "swarm-agent", url: "agent://7", created_at: 0, updated_at: 0, agent: { connected: true, enrolled: true, custom_resources: false }, swarm_nodes_connected: 2 };
const VIEWER = { ...SWARM, access: { role: "viewer", namespaces: null } };

const NODE_N1 = {
  id: "n1", hostname: "mgr-1", role: "manager", state: "ready", availability: "active", leader: true,
  labels: {}, engine: "27.3.1", os: "linux", arch: "x86_64", nano_cpus: 2e9, memory_bytes: 4 * 1024 ** 3, addr: "10.0.0.1", manager_addr: "10.0.0.1:2377", reachability: "reachable",
};
const agent = (node_id: string, hostname: string, role: string, state: string) => ({ node_id, hostname, role, state, connected: true, agent_version: "0.7.0", last_seen_at: 1, last_error: null });

const routes = {
  ...loggedIn,
  "GET /api/v1/envs": { json: [...ENVS, SWARM] },
  "GET /api/v1/envs/prod/status": { json: { state: "up", version: { kind: "swarm", engine: "27.3.1", cluster_id: "c1", managers: 1, workers: 1, serving_node: "n1" } } },
  "GET /api/v1/envs/prod/swarm/agents": { json: [agent("n1", "mgr-1", "manager", "ready"), agent("n2", "wrk-1", "worker", "ready"), agent("n3", "wrk-2", "worker", "down")] },
  "GET /api/v1/envs/prod/swarm/trust": { status: 404, json: { code: "not_found", message: "none" } },
  "GET /api/v1/envs/prod/swarm/nodes/n1": { json: NODE_N1 },
};

const CONFIRM = { status: 409, json: { code: "confirm_required", message: "FleetOps runs on node 'mgr-1'; this can stop FleetOps. Type 'mgr-1' to confirm." } };

describe("Swarm node actions", () => {
  it("drain asks, then types the name when FleetOps runs there", async () => {
    let n = 0;
    const calls = mockFetch({ ...routes, "PATCH /api/v1/envs/prod/swarm/nodes/n1": () => (n++ === 0 ? CONFIRM : { status: 204 }) });
    renderApp("/envs/prod/swarm/nodes/n1");
    await userEvent.click(await screen.findByRole("button", { name: "Actions for mgr-1" }));
    await userEvent.click(within(await screen.findByRole("menu")).getByRole("menuitem", { name: "Drain" }));
    const ask = await screen.findByRole("dialog", { name: "Drain mgr-1?" });
    expect(ask).toHaveTextContent("including its FleetOps agent");
    await userEvent.click(within(ask).getByRole("button", { name: "Drain" }));
    const typed = await screen.findByRole("dialog", { name: "Are you sure?" });
    expect(typed).toHaveTextContent("FleetOps runs on node 'mgr-1'");
    const confirm = within(typed).getByRole("button", { name: "Confirm" });
    expect(confirm).toBeDisabled();
    await userEvent.type(within(typed).getByLabelText(/to confirm/), "mgr-1");
    await userEvent.click(confirm);
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    const patches = calls.filter((c) => c.method === "PATCH");
    expect(patches.map((c) => c.body)).toEqual([{ availability: "drain" }, { availability: "drain", confirm: "mgr-1" }]);
  });

  it("a viewer sees no node actions", async () => {
    mockFetch({ ...routes, "GET /api/v1/envs": { json: [...ENVS, VIEWER] } });
    renderApp("/envs/prod/swarm/nodes");
    expect(await screen.findByRole("link", { name: "wrk-1" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Actions for/ })).not.toBeInTheDocument();
  });

  it("remove is disabled on a ready node", async () => {
    mockFetch(routes);
    renderApp("/envs/prod/swarm/nodes");
    await userEvent.click(await screen.findByRole("button", { name: "Actions for wrk-1" }));
    const item = within(await screen.findByRole("menu")).getByRole("menuitem", { name: /Remove/ });
    expect(item).toHaveAttribute("aria-disabled", "true");
    expect(item).toHaveTextContent("only a node that is down");
  });

  it("labels are sent as a map", async () => {
    const calls = mockFetch({ ...routes, "PATCH /api/v1/envs/prod/swarm/nodes/n1": { status: 204 } });
    renderApp("/envs/prod/swarm/nodes/n1");
    await userEvent.click(await screen.findByRole("button", { name: "Actions for mgr-1" }));
    await userEvent.click(within(await screen.findByRole("menu")).getByRole("menuitem", { name: "Edit labels" }));
    const d = await screen.findByRole("dialog", { name: "Labels on mgr-1" });
    await userEvent.type(within(d).getByLabelText("Labels"), "zone=a{enter}disk = ssd");
    await userEvent.click(within(d).getByRole("button", { name: "Save labels" }));
    await waitFor(() => expect(calls.find((c) => c.method === "PATCH")?.body).toEqual({ labels: { zone: "a", disk: "ssd" } }));
  });
});

const SERVICES = [
  { id: "s1", name: "blog_web", stack: "blog", mode: "replicated", replicas_desired: 3, tasks_running: 3, tasks_desired: 3, image: "nginx:1.27", ports: [], update_state: null, updated_at: 1, labels: {} },
];
const S1 = { ...SERVICES[0], spec: { Name: "blog_web", TaskTemplate: { ContainerSpec: { Image: "nginx:1.27" } }, Mode: { Replicated: { Replicas: 3 } } }, update_status: null, version: 10 };
const svcRoutes = {
  ...routes,
  "GET /api/v1/envs/prod/swarm/services": { json: SERVICES },
  "GET /api/v1/envs/prod/swarm/services/s1": { json: S1 },
};

describe("Swarm service actions", () => {
  it("create posts the built spec", async () => {
    const calls = mockFetch({ ...svcRoutes, "POST /api/v1/envs/prod/swarm/services": { status: 201, json: { id: "s9" } } });
    renderApp("/envs/prod/swarm/services");
    await userEvent.click(await screen.findByRole("button", { name: "Create service" }));
    const d = await screen.findByRole("dialog", { name: "Create service" });
    await userEvent.type(within(d).getByLabelText("Name"), "api");
    await userEvent.type(within(d).getByLabelText("Image"), "nginx:1.28");
    await userEvent.clear(within(d).getByLabelText("Replicas"));
    await userEvent.type(within(d).getByLabelText("Replicas"), "2");
    await userEvent.type(within(d).getByLabelText(/^Ports/), "8080:80");
    await userEvent.click(within(d).getByRole("button", { name: "Create" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(calls.find((c) => c.method === "POST")?.body).toEqual({
      spec: {
        Name: "api",
        TaskTemplate: { ContainerSpec: { Image: "nginx:1.28" } },
        Mode: { Replicated: { Replicas: 2 } },
        EndpointSpec: { Ports: [{ Protocol: "tcp", TargetPort: 80, PublishedPort: 8080 }] },
      },
    });
  });

  it("edit sends the version", async () => {
    const calls = mockFetch({ ...svcRoutes, "PUT /api/v1/envs/prod/swarm/services/s1": { status: 204 } });
    renderApp("/envs/prod/swarm/services/s1");
    await userEvent.click(await screen.findByRole("button", { name: "Edit" }));
    const d = await screen.findByRole("dialog", { name: "Edit blog_web" });
    await userEvent.clear(within(d).getByLabelText("Image"));
    await userEvent.type(within(d).getByLabelText("Image"), "nginx:1.28");
    await userEvent.click(within(d).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    const put = calls.find((c) => c.method === "PUT")?.body as { spec: { TaskTemplate: { ContainerSpec: { Image: string } } }; version: number };
    expect(put.version).toBe(10);
    expect(put.spec.TaskTemplate.ContainerSpec.Image).toBe("nginx:1.28");
  });

  it("raw spec mode refuses invalid JSON", async () => {
    mockFetch(svcRoutes);
    renderApp("/envs/prod/swarm/services/s1");
    await userEvent.click(await screen.findByRole("button", { name: "Edit" }));
    const d = await screen.findByRole("dialog", { name: "Edit blog_web" });
    await userEvent.click(within(d).getByRole("button", { name: "Raw spec" }));
    const raw = within(d).getByLabelText("ServiceSpec (JSON)");
    expect((raw as HTMLTextAreaElement).value).toContain('"Name": "blog_web"');
    await userEvent.clear(raw);
    await userEvent.type(raw, "{{nope");
    await userEvent.click(within(d).getByRole("button", { name: "Save" }));
    expect(await within(d).findByText(/the spec is not valid JSON/)).toBeInTheDocument();
  });

  it("scale with typed confirmation", async () => {
    let n = 0;
    const calls = mockFetch({
      ...svcRoutes,
      "POST /api/v1/envs/prod/swarm/services/s1/scale": () =>
        n++ === 0 ? { status: 409, json: { code: "confirm_required", message: "'blog_web' runs the FleetOps agents; this can disconnect the swarm. Type 'blog_web' to confirm." } } : { status: 204 },
    });
    renderApp("/envs/prod/swarm/services");
    await userEvent.click(await screen.findByRole("button", { name: "Actions for blog_web" }));
    await userEvent.click(within(await screen.findByRole("menu")).getByRole("menuitem", { name: "Scale" }));
    const d = await screen.findByRole("dialog", { name: "Scale blog_web" });
    await userEvent.clear(within(d).getByLabelText("Replicas"));
    await userEvent.type(within(d).getByLabelText("Replicas"), "5");
    await userEvent.click(within(d).getByRole("button", { name: "Scale" }));
    const typed = await screen.findByRole("dialog", { name: "Are you sure?" });
    await userEvent.type(within(typed).getByLabelText(/to confirm/), "blog_web");
    await userEvent.click(within(typed).getByRole("button", { name: "Confirm" }));
    await waitFor(() => expect(calls.filter((c) => c.method === "POST").length).toBe(2));
    expect(calls.filter((c) => c.method === "POST").map((c) => c.body)).toEqual([{ replicas: 5 }, { replicas: 5, confirm: "blog_web" }]);
  });

  it("an operator without swarm-services.create sees no Create button", async () => {
    const limited = { ...SWARM, access: { role: "operator", namespaces: null, grants: [{ namespaces: null, permissions: ["swarm-services.view", "swarm-services.operate"] }] } };
    mockFetch({ ...svcRoutes, "GET /api/v1/envs": { json: [...ENVS, limited] } });
    renderApp("/envs/prod/swarm/services");
    expect(await screen.findByRole("link", { name: "blog_web" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Create service" })).not.toBeInTheDocument();
  });
});

describe("Swarm secrets, configs and networks", () => {
  const objRoutes = {
    ...routes,
    "GET /api/v1/envs/prod/swarm/secrets": { json: [] },
    "GET /api/v1/envs/prod/swarm/configs": { json: [{ id: "cfg1", name: "blog_nginx", stack: null, created_at: 1, updated_at: 1, used_by: [] }] },
    "GET /api/v1/envs/prod/swarm/configs/cfg1/data": { json: { text: "server {}", size: 9 } },
    "GET /api/v1/envs/prod/swarm/networks": { json: [] },
  };

  it("secret create sends the value once and closes", async () => {
    const calls = mockFetch({ ...objRoutes, "POST /api/v1/envs/prod/swarm/secrets": { status: 201, json: { id: "x" } } });
    renderApp("/envs/prod/swarm/secrets");
    await userEvent.click(await screen.findByRole("button", { name: "Create secret" }));
    const d = await screen.findByRole("dialog", { name: "Create secret" });
    expect(d).toHaveTextContent("write-only");
    await userEvent.type(within(d).getByLabelText("Name"), "db_password");
    await userEvent.type(within(d).getByLabelText("Value"), "hunter2");
    await userEvent.click(within(d).getByRole("button", { name: "Create" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(calls.find((c) => c.method === "POST")?.body).toEqual({ name: "db_password", value: "hunter2" });
  });

  it("config content shows text, needs reveal", async () => {
    mockFetch(objRoutes);
    renderApp("/envs/prod/swarm/configs");
    await userEvent.click(await screen.findByRole("button", { name: "Actions for blog_nginx" }));
    await userEvent.click(within(await screen.findByRole("menu")).getByRole("menuitem", { name: "View content" }));
    expect(await screen.findByText("server {}")).toBeInTheDocument();
  });

  it("config content is not offered without reveal", async () => {
    const noReveal = { ...SWARM, access: { role: "operator", namespaces: null, grants: [{ namespaces: null, permissions: ["swarm-configs.view", "swarm-configs.delete"] }] } };
    mockFetch({ ...objRoutes, "GET /api/v1/envs": { json: [...ENVS, noReveal] } });
    renderApp("/envs/prod/swarm/configs");
    await userEvent.click(await screen.findByRole("button", { name: "Actions for blog_nginx" }));
    const menu = await screen.findByRole("menu");
    expect(within(menu).queryByRole("menuitem", { name: "View content" })).not.toBeInTheDocument();
    expect(within(menu).getByRole("menuitem", { name: "Remove" })).toBeInTheDocument();
  });

  it("network create sends the flags", async () => {
    const calls = mockFetch({ ...objRoutes, "POST /api/v1/envs/prod/swarm/networks": { status: 201, json: { id: "n" } } });
    renderApp("/envs/prod/swarm/networks");
    await userEvent.click(await screen.findByRole("button", { name: "Create network" }));
    const d = await screen.findByRole("dialog", { name: "Create network" });
    await userEvent.type(within(d).getByLabelText("Name"), "backend");
    await userEvent.type(within(d).getByLabelText("Subnet (optional)"), "10.20.0.0/24");
    await userEvent.click(within(d).getByRole("checkbox", { name: /Attachable/ }));
    await userEvent.click(within(d).getByRole("checkbox", { name: /Encrypted/ }));
    await userEvent.click(within(d).getByRole("button", { name: "Create" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(calls.find((c) => c.method === "POST")?.body).toEqual({ name: "backend", subnet: "10.20.0.0/24", attachable: true, internal: false, encrypted: true });
  });
});
