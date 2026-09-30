import { act, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { json, mockFetch } from "@/test/fetch";
import { FakeWebSocket } from "@/test/fake-ws";
import { loggedIn, renderApp } from "@/test/app";
import { container } from "@/test/docker";

const LIST = [
  container({
    id: "c1",
    name: "web",
    ports: [{ ip: "0.0.0.0", private_port: 80, public_port: 8080, protocol: "tcp" }],
    compose_project: "shop",
  }),
  container({ id: "c2", name: "db", image: "postgres:17", state: "exited", status: "Exited (0) 1 hour ago" }),
];

const rows = (screen: typeof import("@testing-library/react").screen) =>
  within(screen.getByRole("table", { name: "Containers" })).getAllByRole("row").slice(1);

describe("containers list", () => {
  it("lists containers with state, ports and age", async () => {
    mockFetch({ ...loggedIn, "GET /api/v1/envs/local/containers": { json: LIST } });
    const { screen } = renderApp("/envs/local/containers");
    await waitFor(() => expect(rows(screen)).toHaveLength(2));
    const web = rows(screen).find((r) => r.textContent?.includes("web"))!;
    expect(within(web).getByText("running")).toBeInTheDocument();
    expect(within(web).getByRole("link", { name: "8080→80" })).toHaveAttribute("href", "http://localhost:8080");
    expect(within(web).getByText("2 hours ago")).toBeInTheDocument();
    // Sorted by name: db first.
    expect(rows(screen)[0]).toHaveTextContent("db");
  });

  it("stops the selected containers and reports a failure by name", async () => {
    const calls = mockFetch({
      ...loggedIn,
      "GET /api/v1/envs/local/containers": { json: LIST },
      "POST /api/v1/envs/local/containers/c1/stop": { status: 204 },
      "POST /api/v1/envs/local/containers/c2/stop": () =>
        json(409, { code: "conflict", message: "container is not running" }),
    });
    const { screen } = renderApp("/envs/local/containers");
    await waitFor(() => expect(rows(screen)).toHaveLength(2));
    await userEvent.click(screen.getByRole("checkbox", { name: "Select all" }));
    await userEvent.click(within(screen.getByRole("group", { name: "Selected" })).getByRole("button", { name: "Stop" }));
    expect(await screen.findByText("Stopped 1 of 2 containers. db: container is not running")).toBeInTheDocument();
    expect(calls.filter((c) => c.method === "POST").map((c) => c.path).sort()).toEqual([
      "/api/v1/envs/local/containers/c1/stop",
      "/api/v1/envs/local/containers/c2/stop",
    ]);
    expect(calls.filter((c) => c.path === "/api/v1/envs/local/containers").length).toBeGreaterThan(1);
  });

  it("removes containers with force after confirming", async () => {
    const calls = mockFetch({
      ...loggedIn,
      "GET /api/v1/envs/local/containers": { json: LIST },
      "DELETE /api/v1/envs/local/containers/c1": { status: 204 },
    });
    const { screen } = renderApp("/envs/local/containers");
    await waitFor(() => expect(rows(screen)).toHaveLength(2));
    await userEvent.click(screen.getByRole("checkbox", { name: "Select c1" }));
    await userEvent.click(within(screen.getByRole("group", { name: "Selected" })).getByRole("button", { name: "Remove" }));
    const dialog = await screen.findByRole("dialog", { name: "Remove web?" });
    await userEvent.click(within(dialog).getByLabelText("Force (stop running containers first)"));
    await userEvent.click(within(dialog).getByRole("button", { name: "Remove" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(calls.find((c) => c.method === "DELETE")?.path).toBe("/api/v1/envs/local/containers/c1?force=true&volumes=false");
    expect(await screen.findByText("Removed 1 container")).toBeInTheDocument();
  });

  it("keeps the remove dialog open with Docker's error", async () => {
    mockFetch({
      ...loggedIn,
      "GET /api/v1/envs/local/containers": { json: LIST },
      "DELETE /api/v1/envs/local/containers/c1": () =>
        json(409, { code: "conflict", message: "cannot remove a running container" }),
    });
    const { screen } = renderApp("/envs/local/containers");
    await waitFor(() => expect(rows(screen)).toHaveLength(2));
    await userEvent.click(screen.getByRole("checkbox", { name: "Select c1" }));
    await userEvent.click(within(screen.getByRole("group", { name: "Selected" })).getByRole("button", { name: "Remove" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.click(within(dialog).getByRole("button", { name: "Remove" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("cannot remove a running container");
  });

  it("prunes stopped containers", async () => {
    mockFetch({
      ...loggedIn,
      "GET /api/v1/envs/local/containers": { json: LIST },
      "POST /api/v1/envs/local/containers/prune": { json: { deleted: ["c2"], space_reclaimed: 2_000_000 } },
    });
    const { screen } = renderApp("/envs/local/containers");
    await userEvent.click(await screen.findByRole("button", { name: "Prune" }));
    await userEvent.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "Prune" }));
    expect(await screen.findByText("Removed 1 containers, reclaimed 2.0 MB")).toBeInTheDocument();
  });

  it("refreshes when Docker reports a container change", async () => {
    let list = LIST;
    mockFetch({ ...loggedIn, "GET /api/v1/envs/local/containers": () => json(200, list) });
    const { screen } = renderApp("/envs/local/containers");
    await waitFor(() => expect(rows(screen)).toHaveLength(2));
    list = [...LIST, container({ id: "c3", name: "cache" })];
    const sock = FakeWebSocket.latest();
    act(() => sock.open());
    const sub = sock.frames().find((f) => f.topic === "events").sub;
    act(() => sock.receive({ sub, type: "data", data: { kind: "container", action: "create", id: "c3", attributes: {}, time: 1 } }));
    await waitFor(() => expect(rows(screen)).toHaveLength(3));
  });
});

const stat = (time: number, cpu: number, mem: number) => ({
  time,
  cpu_percent: cpu,
  memory_used: mem,
  memory_limit: 2_000_000_000,
  net_rx: 0,
  net_tx: 0,
  block_read: 0,
  block_write: 0,
  pids: 3,
});

describe("containers list stats", () => {
  it("shows live CPU and memory for running containers, one subscription each", async () => {
    mockFetch({ ...loggedIn, "GET /api/v1/envs/local/containers": { json: LIST } });
    const { screen } = renderApp("/envs/local/containers");
    await waitFor(() => expect(rows(screen)).toHaveLength(2));
    const sock = FakeWebSocket.latest();
    act(() => sock.open());
    await waitFor(() => expect(sock.frames().some((f) => f.topic === "stats")).toBe(true));
    const stats = sock.frames().filter((f) => f.topic === "stats");
    // Only the running container, and once for both its cells.
    expect(stats).toEqual([{ op: "sub", sub: expect.any(Number), topic: "stats", env: "local", id: "c1" }]);
    act(() => {
      sock.receive({ sub: stats[0].sub, type: "data", data: stat(1000, 3.21, 40_000_000) });
      sock.receive({ sub: stats[0].sub, type: "data", data: stat(2000, 12.6, 48_000_000) });
    });
    const web = rows(screen).find((r) => r.textContent?.includes("web"))!;
    expect(await within(web).findByText("13%")).toBeInTheDocument();
    expect(within(web).getByText("48.0 MB")).toBeInTheDocument();
    expect(within(web).getByRole("img", { name: "CPU of web" }).querySelector("polyline")).toBeInTheDocument();
    const db = rows(screen).find((r) => r.textContent?.includes("db"))!;
    expect(within(db).getAllByText("—")).toHaveLength(2);
  });

  it("subscribes only for rows on screen", async () => {
    const observed: { el: Element; cb: IntersectionObserverCallback }[] = [];
    vi.stubGlobal(
      "IntersectionObserver",
      class {
        cb: IntersectionObserverCallback;
        constructor(cb: IntersectionObserverCallback) {
          this.cb = cb;
        }
        observe(el: Element) {
          observed.push({ el, cb: this.cb });
        }
        disconnect() {}
      },
    );
    const many = Array.from({ length: 3 }, (_, i) => container({ id: `c${i}`, name: `app-${i}` }));
    mockFetch({ ...loggedIn, "GET /api/v1/envs/local/containers": { json: many } });
    const { screen } = renderApp("/envs/local/containers");
    await waitFor(() => expect(rows(screen)).toHaveLength(3));
    const sock = FakeWebSocket.latest();
    act(() => sock.open());
    await waitFor(() => expect(observed.length).toBeGreaterThan(0));
    expect(sock.frames().filter((f) => f.topic === "stats")).toEqual([]);
    const row = rows(screen).find((r) => r.textContent?.includes("app-1"))!;
    const cell = observed.find((o) => row.contains(o.el))!;
    act(() => cell.cb([{ isIntersecting: true } as IntersectionObserverEntry], {} as IntersectionObserver));
    await waitFor(() => expect(sock.frames().filter((f) => f.topic === "stats").map((f) => f.id)).toEqual(["c1"]));
  });
});
