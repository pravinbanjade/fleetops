import { act, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { mockFetch } from "@/test/fetch";
import { FakeWebSocket } from "@/test/fake-ws";
import { loggedIn, renderApp } from "@/test/app";
import { container, inspect } from "@/test/docker";

// uPlot draws on a canvas, which jsdom lacks; the tests read what it was given.
const { FakePlot } = vi.hoisted(() => {
  class FakePlot {
    static all: FakePlot[] = [];
    opts: { series: { label?: string }[] };
    data: unknown[];
    destroyed = false;
    constructor(opts: FakePlot["opts"], data: unknown[]) {
      this.opts = opts;
      this.data = data;
      FakePlot.all.push(this);
    }
    setData(d: unknown[]) {
      this.data = d;
    }
    setSize() {}
    destroy() {
      this.destroyed = true;
    }
    static titled(label: string) {
      return FakePlot.all.find((p) => !p.destroyed && p.opts.series[1]?.label === label)!;
    }
  }
  return { FakePlot };
});
vi.mock("uplot", () => ({ default: FakePlot }));

const WEB = inspect({ Id: "c1abcdef0123456789", Name: "/web" });
const STOPPED = inspect({ Id: "c1abcdef0123456789", Name: "/web", State: { Status: "exited", Running: false, ExitCode: 0 } });

const stat = (time: number, over: Record<string, number> = {}) => ({
  time,
  cpu_percent: 5,
  memory_used: 50_000_000,
  memory_limit: 2_000_000_000,
  net_rx: 0,
  net_tx: 0,
  block_read: 0,
  block_write: 0,
  pids: 4,
  ...over,
});

async function statsSub() {
  await waitFor(() => expect(FakeWebSocket.instances.length).toBeGreaterThan(0));
  const sock = FakeWebSocket.latest();
  if (sock.readyState !== FakeWebSocket.OPEN) act(() => sock.open());
  let sub = 0;
  await waitFor(() => {
    sub = sock.frames().find((f) => f.op === "sub" && f.topic === "stats")?.sub;
    expect(sub).toBeTruthy();
  });
  return { sock, sub };
}

describe("stats tab", () => {
  it("shows current usage and charts it as samples arrive", async () => {
    FakePlot.all = [];
    mockFetch({ ...loggedIn, "GET /api/v1/envs/local/containers/c1": { json: WEB } });
    const { screen } = renderApp("/envs/local/containers/c1?tab=stats");
    const { sock, sub } = await statsSub();
    expect(await screen.findByText("Waiting for the first sample…")).toBeInTheDocument();
    act(() => {
      sock.receive({ sub, type: "data", data: stat(1000, { net_rx: 0, block_write: 0 }) });
      sock.receive({
        sub,
        type: "data",
        data: stat(3000, { cpu_percent: 42.4, memory_used: 48_000_000, net_rx: 4000, net_tx: 1000, block_write: 2_000_000 }),
      });
    });
    const now = await screen.findByLabelText("Current usage");
    expect(within(now).getByText("42%")).toBeInTheDocument();
    expect(within(now).getByText("48.0 MB of 2.0 GB (2.4%)")).toBeInTheDocument();
    expect(within(now).getByText("2.0 KB/s in, 500 B/s out")).toBeInTheDocument();
    expect(within(now).getByText("0 B/s read, 1.0 MB/s written")).toBeInTheDocument();
    expect(within(now).getByText("4")).toBeInTheDocument();
    for (const title of ["CPU", "Memory", "Network", "Block I/O"]) {
      expect(screen.getByRole("img", { name: `${title} chart` })).toBeInTheDocument();
    }
    await waitFor(() => expect(FakePlot.titled("CPU").data).toEqual([[1, 3], [5, 42.4]]));
    expect(FakePlot.titled("In").data).toEqual([
      [1, 3],
      [null, 2000],
      [null, 500],
    ]);
  });

  it("keeps the history gathered on the list when opening a container", async () => {
    FakePlot.all = [];
    mockFetch({
      ...loggedIn,
      "GET /api/v1/envs/local/containers": { json: [container({ id: "c1", name: "web" })] },
      "GET /api/v1/envs/local/containers/c1": { json: WEB },
    });
    const { screen } = renderApp("/envs/local/containers");
    const { sock, sub } = await statsSub();
    act(() => sock.receive({ sub, type: "data", data: stat(1000, { cpu_percent: 7 }) }));
    expect(await screen.findByText("7.0%")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("link", { name: "web" }));
    await userEvent.click(await screen.findByRole("tab", { name: "Stats" }));
    const now = await screen.findByLabelText("Current usage");
    expect(within(now).getByText("7.0%")).toBeInTheDocument();
    expect(sock.frames().filter((f) => f.op === "sub" && f.topic === "stats")).toHaveLength(1);
  });

  it("says when the container is not running and does not subscribe", async () => {
    mockFetch({ ...loggedIn, "GET /api/v1/envs/local/containers/c1": { json: STOPPED } });
    const { screen } = renderApp("/envs/local/containers/c1?tab=stats");
    expect(await screen.findByText("The container is not running.")).toBeInTheDocument();
    const sock = FakeWebSocket.latest();
    act(() => sock.open());
    await waitFor(() => expect(sock.frames().some((f) => f.topic === "events")).toBe(true));
    expect(sock.frames().some((f) => f.topic === "stats")).toBe(false);
  });
});
