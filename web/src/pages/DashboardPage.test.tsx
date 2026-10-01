import { act, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { ContainerSummary, DockerEvent } from "@/api/gen/types.gen";
import { json, mockFetch } from "@/test/fetch";
import { FakeWebSocket } from "@/test/fake-ws";
import { loggedIn, renderApp } from "@/test/app";

const info = {
  Containers: 3,
  ContainersRunning: 2,
  ContainersPaused: 0,
  ContainersStopped: 1,
  Images: 5,
  ServerVersion: "29.5.2",
  OperatingSystem: "Ubuntu 24.04",
  Architecture: "aarch64",
  NCPU: 2,
  MemTotal: 2_000_000_000,
};

const container = (over: Partial<ContainerSummary>): ContainerSummary => ({
  id: "c1",
  name: "web",
  image: "nginx:1.27",
  image_id: "sha256:n",
  state: "running",
  status: "Up 2 hours",
  created: 0,
  ports: [],
  labels: {},
  volumes: [],
  ...over,
});

const HEALTHY = [
  container({ id: "c1", name: "web" }),
  container({ id: "c2", name: "db" }),
  container({ id: "c3", name: "migrate", state: "exited", status: "Exited (0) 3 hours ago" }),
];

const event = (over: Partial<DockerEvent>): DockerEvent => ({
  kind: "container",
  action: "start",
  id: "c1",
  name: "web",
  attributes: {},
  time: 1_790_000_000,
  ...over,
});

/** The events subscription the dashboard's shell opened, once the socket is open. */
async function eventsSub() {
  await waitFor(() => expect(FakeWebSocket.instances.length).toBeGreaterThan(0));
  const sock = FakeWebSocket.latest();
  act(() => sock.open());
  const sub = sock.frames().find((f) => f.op === "sub" && f.topic === "events");
  expect(sub).toMatchObject({ env: "local" });
  return { sock, sub: sub.sub as number };
}

describe("dashboard status", () => {
  it("says when nothing needs attention, and counts containers by state with links to them", async () => {
    mockFetch({ ...loggedIn, "GET /api/v1/envs/local/containers": { json: HEALTHY } });
    const { screen } = renderApp("/envs/local");
    expect(await screen.findByRole("heading", { name: "Nothing needs attention" })).toBeInTheDocument();
    const byState = screen.getByRole("list", { name: "Containers by state" });
    expect(within(byState).getByRole("link", { name: "2 running" })).toHaveAttribute(
      "href",
      "/envs/local/containers?q=running",
    );
    expect(within(byState).getByRole("link", { name: "1 exited" })).toHaveAttribute(
      "href",
      "/envs/local/containers?q=exited",
    );
    expect(screen.queryByRole("list", { name: "Needs attention" })).not.toBeInTheDocument();
  });

  it("names the containers that need attention, first, with Docker's own status", async () => {
    mockFetch({
      ...loggedIn,
      "GET /api/v1/envs/local/containers": {
        json: [
          ...HEALTHY,
          container({ id: "c4", name: "api", status: "Up 5 minutes (unhealthy)" }),
          container({ id: "c5", name: "worker", state: "restarting", status: "Restarting (1) 4 seconds ago" }),
          container({ id: "c6", name: "cron", state: "exited", status: "Exited (137) 2 minutes ago" }),
        ],
      },
    });
    const { screen } = renderApp("/envs/local");
    expect(await screen.findByRole("heading", { name: "3 containers need attention" })).toBeInTheDocument();
    const list = screen.getByRole("list", { name: "Needs attention" });
    const items = within(list).getAllByRole("listitem");
    expect(items.map((li) => li.textContent)).toEqual([
      expect.stringContaining("api"),
      expect.stringContaining("worker"),
      expect.stringContaining("cron"),
    ]);
    expect(items[0]).toHaveTextContent("Up 5 minutes (unhealthy)");
    expect(items[2]).toHaveTextContent("Exited (137) 2 minutes ago");
    expect(within(items[1]).getByRole("link", { name: "worker" })).toHaveAttribute(
      "href",
      "/envs/local/containers/c5?tab=logs",
    );
    // An exited job with code 0 is not a problem.
    expect(within(list).queryByText("migrate")).not.toBeInTheDocument();
    const byState = screen.getByRole("list", { name: "Containers by state" });
    expect(within(byState).getByRole("link", { name: "1 unhealthy" })).toHaveAttribute(
      "href",
      "/envs/local/containers?q=unhealthy",
    );
  });

  it("says so when the environment has no containers", async () => {
    mockFetch({ ...loggedIn, "GET /api/v1/envs/local/containers": { json: [] } });
    const { screen } = renderApp("/envs/local");
    expect(await screen.findByRole("heading", { name: "No containers yet" })).toBeInTheDocument();
  });

  it("shows Docker's message when the container list fails, instead of a count", async () => {
    mockFetch({
      ...loggedIn,
      "GET /api/v1/envs/local/containers": { status: 502, json: { code: "upstream", message: "Docker said no" } },
    });
    const { screen } = renderApp("/envs/local");
    expect(await screen.findByText("Docker said no", {}, { timeout: 3000 })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Nothing needs attention" })).not.toBeInTheDocument();
  });

  it("names the page in the browser tab", async () => {
    mockFetch(loggedIn);
    renderApp("/envs/local");
    await waitFor(() => expect(document.title).toBe("local · FleetOps"));
  });

  it("tells the user it is live, and when it is not", async () => {
    mockFetch({ ...loggedIn, "GET /api/v1/envs/local/containers": { json: HEALTHY } });
    const { screen } = renderApp("/envs/local");
    await screen.findByRole("heading", { name: "Nothing needs attention" });
    const { sock } = await eventsSub();
    const liveness = screen.getByTestId("liveness");
    await waitFor(() => expect(liveness).toHaveTextContent("Live"));
    act(() => sock.drop());
    await waitFor(() => expect(liveness).toHaveTextContent(/Reconnecting.*as of \d/));
  });
});

describe("dashboard host", () => {
  it("shows host details and live CPU and memory summed over running containers", async () => {
    mockFetch({
      ...loggedIn,
      "GET /api/v1/envs/local/info": { json: info },
      "GET /api/v1/envs/local/containers": { json: HEALTHY },
    });
    const { screen } = renderApp("/envs/local");
    expect(await screen.findByText(/29\.5\.2/)).toBeInTheDocument();
    const sock = FakeWebSocket.latest();
    act(() => sock.open());
    await waitFor(() => expect(sock.frames().filter((f) => f.topic === "stats").map((f) => f.id).sort()).toEqual(["c1", "c2"]));
    const stats = sock.frames().filter((f) => f.topic === "stats");
    const sample = (cpu: number, mem: number) => ({
      time: 1000,
      cpu_percent: cpu,
      memory_used: mem,
      memory_limit: 2_000_000_000,
      net_rx: 0,
      net_tx: 0,
      block_read: 0,
      block_write: 0,
      pids: 1,
    });
    act(() => {
      sock.receive({ sub: stats[0].sub, type: "data", data: sample(30, 300_000_000) });
      sock.receive({ sub: stats[1].sub, type: "data", data: sample(20, 200_000_000) });
    });
    // 50% of one core, on a 2-CPU host, is 25% of the host.
    const cpu = await screen.findByRole("group", { name: "CPU" });
    await waitFor(() => expect(cpu).toHaveTextContent("25%"));
    expect(cpu).toHaveTextContent("of 2 CPUs");
    const memory = screen.getByRole("group", { name: "Memory" });
    expect(memory).toHaveTextContent("500.0 MB");
    expect(memory).toHaveTextContent("of 2.0 GB");
    expect(screen.getByRole("img", { name: "CPU over the last minute" })).toBeInTheDocument();
  });

  it("shows disk usage by kind with what can be reclaimed, linked to where it can be pruned", async () => {
    mockFetch({
      ...loggedIn,
      "GET /api/v1/envs/local/df": { json: { ImageUsage: { TotalSize: 3_000_000_000, Reclaimable: 1_000_000_000 } } },
    });
    const { screen } = renderApp("/envs/local");
    const disk = await screen.findByRole("list", { name: "Disk usage" });
    const images = await within(disk).findByRole("listitem", { name: "Images" });
    expect(images).toHaveTextContent("3.0 GB");
    expect(images).toHaveTextContent("1.0 GB reclaimable");
    expect(within(images).getByRole("link", { name: "Images" })).toHaveAttribute("href", "/envs/local/images");
  });

  it("shows Docker's message when disk usage fails", async () => {
    mockFetch({
      ...loggedIn,
      "GET /api/v1/envs/local/df": { status: 502, json: { code: "upstream", message: "df timed out" } },
    });
    const { screen } = renderApp("/envs/local");
    expect(await screen.findByText("df timed out", {}, { timeout: 3000 })).toBeInTheDocument();
  });

  it("counts images, volumes and networks as links", async () => {
    mockFetch({
      ...loggedIn,
      "GET /api/v1/envs/local/info": { json: info },
      "GET /api/v1/envs/local/volumes": { json: [{ name: "v1" }, { name: "v2" }] },
    });
    const { screen } = renderApp("/envs/local");
    expect(await screen.findByRole("link", { name: "5 images" })).toHaveAttribute("href", "/envs/local/images");
    expect(await screen.findByRole("link", { name: "2 volumes" })).toHaveAttribute("href", "/envs/local/volumes");
    expect(screen.getByRole("link", { name: "0 networks" })).toHaveAttribute("href", "/envs/local/networks");
  });
});

describe("dashboard events", () => {
  it("shows what Docker still holds from the last 15 minutes, newest first, when the page opens", async () => {
    let since = "";
    mockFetch({
      ...loggedIn,
      "GET /api/v1/envs/local/events": (call) => {
        since = new URL(call.path, "http://x").searchParams.get("since") ?? "";
        return json(200, [
          event({ action: "start", time: 1_790_000_000 }),
          event({ action: "exec_start: sh -c healthcheck", time: 1_790_000_001 }),
          event({ action: "die", id: "c6", name: "cron", attributes: { exitCode: "137" }, time: 1_790_000_002 }),
        ]);
      },
    });
    const now = Math.floor(Date.now() / 1000);
    const { screen } = renderApp("/envs/local");
    const feed = await screen.findByRole("list", { name: "Recent events" });
    const rows = await within(feed).findAllByRole("listitem");
    expect(Math.abs(Number(since) - (now - 900))).toBeLessThan(5);
    // Newest first; healthcheck execs are noise.
    expect(rows).toHaveLength(2);
    expect(rows[0]).toHaveTextContent("container die");
    expect(rows[0]).toHaveTextContent("exit code 137");
    expect(within(rows[0]).getByRole("link", { name: "cron" })).toHaveAttribute("href", "/envs/local/containers/c6");
    expect(rows[1]).toHaveTextContent("container start");
    expect(screen.getByText("Last 15 minutes")).toBeInTheDocument();
  });

  it("adds live events above the ones it loaded, and keeps counts live", async () => {
    let list = HEALTHY;
    mockFetch({
      ...loggedIn,
      "GET /api/v1/envs/local/containers": () => json(200, list),
      "GET /api/v1/envs/local/events": { json: [event({ action: "start", name: "web" })] },
    });
    const { screen } = renderApp("/envs/local");
    const feed = await screen.findByRole("list", { name: "Recent events" });
    await within(feed).findByText("container start");
    const { sock, sub } = await eventsSub();
    list = [...HEALTHY, container({ id: "c7", name: "cache" })];
    act(() => sock.receive({ sub, type: "data", data: event({ action: "create", id: "c7", name: "cache", time: 1_790_000_100 }) }));
    await waitFor(() => expect(within(feed).getAllByRole("listitem")[0]).toHaveTextContent("container create"));
    const byState = screen.getByRole("list", { name: "Containers by state" });
    await waitFor(() => expect(within(byState).getByRole("link", { name: "3 running" })).toBeInTheDocument());
  });

  it("says when the event history could not be loaded", async () => {
    mockFetch({
      ...loggedIn,
      "GET /api/v1/envs/local/events": { status: 502, json: { code: "upstream", message: "events unavailable" } },
    });
    const { screen } = renderApp("/envs/local");
    expect(await screen.findByText(/events unavailable/, {}, { timeout: 3000 })).toBeInTheDocument();
  });
});

describe("dashboard when the environment is down", () => {
  it("explains why, when it will check again, and where to manage it", async () => {
    mockFetch({ ...loggedIn, "GET /api/v1/envs/local/status": { json: { state: "down", reason: "socket not found" } } });
    const { screen } = renderApp("/envs/local");
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("This environment is unavailable");
    expect(alert).toHaveTextContent("socket not found");
    expect(alert).toHaveTextContent("Checking again every 5 seconds");
    expect(within(alert).getByRole("link", { name: "Manage environments" })).toHaveAttribute("href", "/admin/envs");
  });

  it("keeps the last known state on screen, marked as not current", async () => {
    mockFetch({
      ...loggedIn,
      "GET /api/v1/envs/local/info": { json: info },
      "GET /api/v1/envs/local/containers": { json: HEALTHY },
    });
    const { screen } = renderApp("/envs/local");
    await screen.findByRole("heading", { name: "Nothing needs attention" });
    const { sock } = await eventsSub();
    act(() => sock.receive({ type: "env_status", env: "local", status: { state: "down", reason: "daemon stopped" } }));
    expect(await screen.findByRole("alert")).toHaveTextContent("daemon stopped");
    expect(screen.getByRole("heading", { name: "Nothing needs attention" })).toBeInTheDocument();
    expect(screen.getByTestId("liveness")).toHaveTextContent(/Unavailable.*last known state.*as of \d/);
  });

  it("recovers when the environment comes back up", async () => {
    let up = false;
    const UP = { state: "up", version: { version: "29.5.2", api_version: "1.52", os: "linux", arch: "arm64" } };
    mockFetch({
      ...loggedIn,
      "GET /api/v1/envs/local/status": () => json(200, up ? UP : { state: "down", reason: "socket not found" }),
      "GET /api/v1/envs/local/info": () =>
        up ? json(200, info) : json(503, { code: "env_unavailable", message: "docker is down" }),
      "GET /api/v1/envs/local/containers": () =>
        up ? json(200, HEALTHY) : json(503, { code: "env_unavailable", message: "docker is down" }),
    });
    const { screen } = renderApp("/envs/local");
    expect(await screen.findByRole("alert")).toHaveTextContent("socket not found");
    await waitFor(() => expect(FakeWebSocket.instances.length).toBeGreaterThan(0));
    const sock = FakeWebSocket.latest();
    act(() => sock.open());
    // The server refuses an events subscription while the environment is down.
    for (const f of sock.frames().filter((f) => f.op === "sub" && f.topic === "events")) {
      act(() => sock.receive({ sub: f.sub, type: "error", code: "env_unavailable", message: "docker is down" }));
    }
    const before = sock.frames().length;
    up = true;
    act(() => sock.receive({ type: "env_status", env: "local", status: UP }));
    expect(await screen.findByRole("heading", { name: "Nothing needs attention" }, { timeout: 3000 })).toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    await waitFor(() =>
      expect(sock.frames().slice(before).some((f) => f.op === "sub" && f.topic === "events" && f.env === "local")).toBe(true),
    );
  });
});
