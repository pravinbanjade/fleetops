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

const STACK = { stack_yaml: "services:\n  agent:\n    deploy:\n      mode: global\n", deploy_command: "docker stack deploy -c fleetops-agent.yml fleetops-agent" };

const enrollment = (key: string, generation = 1, inside = false) => ({
  agent_key: key,
  key_generation: generation,
  secret_name: generation === 1 ? "fleetops-agent-key" : `fleetops-agent-key-${generation}`,
  secret_command: `docker secret create ${generation === 1 ? "fleetops-agent-key" : `fleetops-agent-key-${generation}`} -`,
  outside: STACK,
  inside: inside ? { ...STACK, stack_yaml: "https://fleetops:9443" } : null,
  inside_unavailable: inside ? null : "FleetOps is not serving its generated certificate on an HTTPS port (--tls-bind without --tls-cert)",
});

const routes = {
  ...loggedIn,
  "GET /api/v1/envs": { json: [...ENVS, SWARM] },
  "GET /api/v1/envs/prod/status": {
    json: { state: "up", version: { kind: "swarm", engine: "27.3.1", cluster_id: "c1", managers: 1, workers: 2, serving_node: "n1" } },
  },
  "GET /api/v1/envs/prod/swarm/agents": {
    json: [
      { node_id: "n1", hostname: "mgr-1", role: "manager", state: "ready", connected: true, agent_version: "0.7.0", last_seen_at: 1, last_error: null },
      { node_id: "n2", hostname: "wrk-1", role: "worker", state: "ready", connected: true, agent_version: "0.7.0", last_seen_at: 1, last_error: null },
      { node_id: "n3", hostname: "wrk-2", role: "worker", state: "ready", connected: false, agent_version: null, last_seen_at: null, last_error: null },
    ],
  },
};

describe("Swarm environments", () => {
  it("swarm environments get the swarm sidebar", async () => {
    mockFetch(routes);
    const { screen } = renderApp("/envs/prod");
    const nav = await screen.findByRole("navigation", { name: "Environment" });
    expect(within(nav).getByRole("link", { name: /Nodes/ })).toHaveAttribute("href", "/envs/prod");
    expect(within(nav).queryByRole("link", { name: /Containers/ })).not.toBeInTheDocument();
  });

  it("docker pages redirect home on a swarm environment", async () => {
    mockFetch(routes);
    const { history } = renderApp("/envs/prod/containers");
    await waitFor(() => expect(history.location.pathname).toBe("/envs/prod"));
  });

  it("agents table marks nodes without agents", async () => {
    mockFetch(routes);
    const { screen } = renderApp("/envs/prod");
    const table = await screen.findByRole("table", { name: "Nodes" });
    await waitFor(() => expect(within(table).getAllByRole("row")).toHaveLength(4));
    const wrk2 = within(table).getAllByRole("row").find((r) => r.textContent?.includes("wrk-2"))!;
    expect(wrk2).toHaveTextContent("no agent");
    expect(screen.getByText("2 of 3 nodes have an agent")).toBeInTheDocument();
  });

  it("the enrollment dialog shows the key once and hides inside when unavailable", async () => {
    const calls = mockFetch({
      ...routes,
      "POST /api/v1/envs": { status: 201, json: { ...SWARM, agent: undefined, enrollment: null, swarm_enrollment: enrollment("fk_secret") } },
    });
    const { screen } = renderApp("/admin/envs");
    await userEvent.click(await screen.findByRole("button", { name: "Add environment" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.click(within(dialog).getByRole("radio", { name: "Docker Swarm" }));
    await userEvent.type(within(dialog).getByLabelText("Name"), "prod");
    await userEvent.click(within(dialog).getByRole("button", { name: "Add" }));
    expect(calls.find((c) => c.method === "POST")?.body).toEqual({ name: "prod", kind: "swarm-agent" });
    const shown = await screen.findByRole("dialog", { name: /start the agents on prod/i });
    expect(within(shown).getByText("fk_secret")).toBeInTheDocument();
    expect(within(shown).getByText("docker secret create fleetops-agent-key -")).toBeInTheDocument();
    expect(within(shown).getByText(/mode: global/)).toBeInTheDocument();
    const inside = within(shown).getByRole("radio", { name: /inside the swarm/i });
    expect(inside).toBeDisabled();
    expect(within(shown).getByText(/--tls-bind/)).toBeInTheDocument();
    await userEvent.click(within(shown).getByRole("button", { name: "Done" }));
    await waitFor(() => expect(screen.queryByText("fk_secret")).not.toBeInTheDocument());
  });

  it("finish rotation is offered after a rotate", async () => {
    const calls = mockFetch({
      ...routes,
      "POST /api/v1/envs/prod/swarm/rotate": { json: enrollment("fk_new", 2, true) },
      "POST /api/v1/envs/prod/swarm/rotate/finish": { status: 204 },
    });
    const { screen } = renderApp("/admin/envs");
    const table = await screen.findByRole("table", { name: "Environments" });
    const row = await waitFor(() => within(table).getAllByRole("row").find((r) => r.textContent?.includes("prod"))!);
    expect(row).toHaveTextContent("2 nodes connected");
    await userEvent.click(within(row).getByRole("button", { name: "Rotate key" }));
    const confirm = await screen.findByRole("dialog");
    expect(confirm).toHaveTextContent(/keeps working for 24 hours/i);
    await userEvent.click(within(confirm).getByRole("button", { name: "Rotate" }));
    const shown = await screen.findByRole("dialog", { name: /start the agents on prod/i });
    expect(within(shown).getByText("docker secret create fleetops-agent-key-2 -")).toBeInTheDocument();
    await userEvent.click(within(shown).getByRole("radio", { name: /inside the swarm/i }));
    expect(within(shown).getByText(/fleetops:9443/)).toBeInTheDocument();
    await userEvent.click(within(shown).getByRole("button", { name: "Finish rotation" }));
    await waitFor(() => expect(calls.some((c) => c.method === "POST" && c.path.endsWith("/swarm/rotate/finish"))).toBe(true));
  });
});
