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
