import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { FakeWebSocket } from "@/test/fake-ws";
import { mockFetch } from "@/test/fetch";
import { ENVS, loggedIn, renderApp } from "@/test/app";

const SWARM = { id: 7, name: "prod", kind: "swarm-agent", url: "agent://7", created_at: 0, updated_at: 0, agent: { connected: true, enrolled: true, custom_resources: false }, swarm_nodes_connected: 2 };

const node = (id: string, hostname: string, role: string, state = "ready") => ({ id, hostname, role, state, availability: "active", leader: id === "n1" });
const NODES = [node("n1", "mgr-1", "manager"), node("n2", "wrk-1", "worker"), node("n3", "wrk-2", "worker", "down")];
const SERVICES = [
  { id: "s1", name: "blog_web", stack: "blog", mode: "replicated", replicas_desired: 3, tasks_running: 2, tasks_desired: 3, image: "nginx:1.27", ports: ["8080:80/tcp"], update_state: null, updated_at: 1 },
  { id: "s2", name: "agent", stack: null, mode: "global", replicas_desired: null, tasks_running: 3, tasks_desired: 3, image: "fleets/fleetops-agent:0.7.0", ports: [], update_state: null, updated_at: 1 },
];
const now = Math.floor(Date.now() / 1000);
const task = (id: string, svc: string, nodeId: string, state: string, extra: object = {}) => ({
  id, service_id: svc, service_name: svc === "s1" ? "blog_web" : "agent", slot: 1, node_id: nodeId, desired_state: "running", state, message: "", error: null, container_id: `c-${id}`, updated_at: now, version: 1, ...extra,
});
const TASKS = [task("t1", "s1", "n1", "running"), task("t2", "s1", "n2", "running"), task("t3", "s1", "n3", "rejected", { error: "No such image: nginx:9", container_id: null, desired_state: "shutdown" })];

const routes = {
  ...loggedIn,
  "GET /api/v1/envs": { json: [...ENVS, SWARM] },
  "GET /api/v1/envs/prod/status": { json: { state: "up", version: { kind: "swarm", engine: "27.3.1", cluster_id: "c1", managers: 1, workers: 2, serving_node: "n1" } } },
  "GET /api/v1/envs/prod/swarm/agents": { json: [] },
  "GET /api/v1/envs/prod/swarm/nodes": { json: NODES },
  "GET /api/v1/envs/prod/swarm/services": { json: SERVICES },
  "GET /api/v1/envs/prod/swarm/services/s1": { json: { ...SERVICES[0], labels: {}, spec: { Name: "blog_web" }, update_status: null } },
  "GET /api/v1/envs/prod/swarm/secrets": { json: [{ id: "sec1", name: "blog_db", stack: "blog", created_at: 1, updated_at: 1, used_by: ["blog_web"] }] },
};

/** Opens the fake socket and finds the subscription for `topic`. */
async function subOf(topic: string, id?: string) {
  let frame: { sub: number } | undefined;
  await vi.waitFor(() => {
    const sock = FakeWebSocket.latest();
    if (sock.readyState !== FakeWebSocket.OPEN) act(() => sock.open());
    frame = sock.frames().find((f) => f.topic === topic && f.env === "prod" && (id === undefined || f.id === id));
    expect(frame).toBeDefined();
  });
  return { sock: FakeWebSocket.latest(), sub: frame!.sub };
}

async function feedTasks() {
  const { sock, sub } = await subOf("swarm-tasks");
  act(() => sock.receive({ sub, type: "data", data: { type: "snapshot", tasks: TASKS } }));
  return { sock, sub };
}

describe("Swarm cluster pages", () => {
  it("the overview leads with what needs attention", async () => {
    mockFetch(routes);
    renderApp("/envs/prod");
    await feedTasks();
    const attention = await screen.findByRole("region", { name: "Needs attention" });
    await waitFor(() => expect(attention).toHaveTextContent("wrk-2 is down"));
    expect(attention).toHaveTextContent("blog_web has 2 of 3 tasks running");
    expect(attention).toHaveTextContent("No such image: nginx:9");
    expect(screen.getByText(/Serving manager/)).toHaveTextContent("mgr-1");
  });

  it("the overview counts running tasks from the task feed", async () => {
    mockFetch(routes);
    renderApp("/envs/prod");
    const { sock, sub } = await feedTasks();
    const attention = await screen.findByRole("region", { name: "Needs attention" });
    await waitFor(() => expect(attention).toHaveTextContent("blog_web has 2 of 3 tasks running"));
    act(() => sock.receive({ sub, type: "data", data: { type: "changes", upsert: [task("t4", "s1", "n2", "running")], remove: [] } }));
    await waitFor(() => expect(attention).not.toHaveTextContent("blog_web has"));
  });

  it("the overview says when the task feed is paused", async () => {
    mockFetch(routes);
    renderApp("/envs/prod");
    const { sock, sub } = await feedTasks();
    act(() => sock.receive({ sub, type: "data", data: { type: "paused", reason: "no manager agent is connected" } }));
    expect(await screen.findByText(/Live updates paused/)).toHaveTextContent("no manager agent is connected");
  });

  it("the visualizer places tasks on their nodes", async () => {
    mockFetch(routes);
    renderApp("/envs/prod/swarm/visualizer");
    await feedTasks();
    const col = await screen.findByRole("group", { name: "mgr-1" });
    expect(within(col).getByText("blog_web.1")).toBeInTheDocument();
    expect(within(screen.getByRole("group", { name: "wrk-2" })).queryByText("blog_web.1")).not.toBeInTheDocument();
  });

  it("a task links to its container on its node", async () => {
    mockFetch(routes);
    renderApp("/envs/prod/swarm/tasks");
    await feedTasks();
    const table = await screen.findByRole("table", { name: "Tasks" });
    const link = await within(table).findByRole("link", { name: "c-t1" });
    expect(link).toHaveAttribute("href", "/envs/prod%40n1/containers/c-t1");
  });

  it("service page shows tasks and streams logs", async () => {
    mockFetch(routes);
    renderApp("/envs/prod/swarm/services/s1");
    expect(await screen.findByRole("heading", { name: "blog_web" })).toBeInTheDocument();
    await feedTasks();
    const tasks = await screen.findByRole("table", { name: "Tasks" });
    await waitFor(() => expect(within(tasks).getAllByRole("row")).toHaveLength(4));
    await userEvent.click(screen.getByRole("tab", { name: /Logs/ }));
    await subOf("logs", "service/s1");
  });

  it("secrets show who uses them and never values", async () => {
    mockFetch(routes);
    renderApp("/envs/prod/swarm/secrets");
    const table = await screen.findByRole("table", { name: "Secrets" });
    await waitFor(() => expect(table).toHaveTextContent("blog_web"));
    expect(table).not.toHaveTextContent(/value/i);
  });

  it("swarm events refetch lists", async () => {
    const calls = mockFetch(routes);
    renderApp("/envs/prod/swarm/services");
    await screen.findByRole("table", { name: "Services" });
    const { sock, sub } = await subOf("events");
    const before = calls.filter((c) => c.path === "/api/v1/envs/prod/swarm/services").length;
    act(() => sock.receive({ sub, type: "data", data: { kind: "service", action: "update", id: "s1", name: "blog_web", attributes: {}, time: now } }));
    await waitFor(() => expect(calls.filter((c) => c.path === "/api/v1/envs/prod/swarm/services").length).toBeGreaterThan(before));
  });
});

describe("Swarm task feed", () => {
  it("starts over after falling behind", async () => {
    mockFetch(routes);
    renderApp("/envs/prod/swarm/tasks");
    const { sock, sub } = await feedTasks();
    act(() => sock.receive({ sub, type: "end", reason: "lagged" }));
    await vi.waitFor(() => {
      const again = sock.frames().filter((f) => f.topic === "swarm-tasks" && f.op === "sub");
      expect(again.length).toBeGreaterThan(1);
    });
    expect(screen.queryByText(/Live updates paused/)).not.toBeInTheDocument();
  });
});
