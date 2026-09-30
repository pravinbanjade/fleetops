import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { StatsSample } from "@/api/gen/types.gen";
import { FakeWebSocket } from "@/test/fake-ws";
import { StatsStore, STATS_HISTORY, STATS_RETRY_MS, toPoint } from "./stats";
import { WsClient } from "./ws";

const sample = (time: number, over: Partial<StatsSample> = {}): StatsSample => ({
  time,
  cpu_percent: 1,
  memory_used: 100,
  memory_limit: 1000,
  net_rx: 0,
  net_tx: 0,
  block_read: 0,
  block_write: 0,
  pids: 1,
  ...over,
});

describe("toPoint", () => {
  it("turns running totals into per-second rates", () => {
    const p = toPoint(sample(3000, { net_rx: 5000, block_write: 800 }), sample(1000, { net_rx: 1000, block_write: 0 }));
    expect(p.net_rx_rate).toBe(2000);
    expect(p.block_write_rate).toBe(400);
  });

  it("has no rate for the first sample and never a negative one", () => {
    expect(toPoint(sample(1000), undefined).net_rx_rate).toBeNull();
    expect(toPoint(sample(2000, { net_tx: 10 }), sample(1000, { net_tx: 900 })).net_tx_rate).toBe(0);
    expect(toPoint(sample(1000), sample(1000)).net_tx_rate).toBeNull();
  });
});

describe("StatsStore", () => {
  let ws: WsClient;
  let sock: FakeWebSocket;
  beforeEach(() => {
    vi.useFakeTimers();
    FakeWebSocket.instances = [];
    ws = new WsClient({ url: "ws://test", WebSocketImpl: FakeWebSocket as unknown as typeof WebSocket });
    ws.start();
    sock = FakeWebSocket.latest();
    sock.open();
  });
  afterEach(() => vi.useRealTimers());

  const subs = () => sock.frames().filter((f) => f.op === "sub" && f.topic === "stats");

  it("shares one subscription between readers and keeps a bounded history", () => {
    const store = new StatsStore(ws);
    const a = vi.fn();
    const b = vi.fn();
    store.subscribe("local", "c1", a);
    store.subscribe("local", "c1", b);
    expect(subs()).toEqual([{ op: "sub", sub: 1, topic: "stats", env: "local", id: "c1" }]);
    for (let i = 0; i < STATS_HISTORY + 5; i++) sock.receive({ sub: 1, type: "data", data: sample(i * 1000) });
    expect(a).toHaveBeenCalledTimes(STATS_HISTORY + 5);
    expect(b).toHaveBeenCalledTimes(STATS_HISTORY + 5);
    const points = store.get("local", "c1");
    expect(points).toHaveLength(STATS_HISTORY);
    expect(points.at(-1)!.time).toBe((STATS_HISTORY + 4) * 1000);
    expect(store.get("local", "c1")).toBe(points);
  });

  it("keeps the subscription through a brief gap between readers, then ends it", () => {
    const store = new StatsStore(ws, 1000);
    const stop = store.subscribe("local", "c1", () => {});
    sock.receive({ sub: 1, type: "data", data: sample(1000) });
    stop();
    vi.advanceTimersByTime(500);
    store.subscribe("local", "c1", () => {})();
    expect(subs()).toHaveLength(1);
    expect(store.get("local", "c1")).toHaveLength(1);
    vi.advanceTimersByTime(1000);
    expect(sock.frames().at(-1)).toEqual({ op: "unsub", sub: 1 });
    expect(store.get("local", "c1")).toHaveLength(0);
  });

  it("subscribes again for a new reader after the stream ended", () => {
    const store = new StatsStore(ws);
    const stop = store.subscribe("local", "c1", () => {});
    sock.receive({ sub: 1, type: "end", reason: "ended" });
    stop();
    store.subscribe("local", "c1", () => {});
    expect(subs().map((f) => f.sub)).toEqual([1, 2]);
  });

  it("reopens a stream that ended while it still has readers, as after a quick restart", () => {
    const store = new StatsStore(ws);
    store.subscribe("local", "c1", () => {});
    sock.receive({ sub: 1, type: "end", reason: "ended" });
    expect(subs()).toHaveLength(1);
    vi.advanceTimersByTime(STATS_RETRY_MS);
    expect(subs().map((f) => f.sub)).toEqual([1, 2]);
  });

  it("does not reopen for readers that left, or after a refusal", () => {
    const store = new StatsStore(ws, 0);
    const stop = store.subscribe("local", "c1", () => {});
    sock.receive({ sub: 1, type: "end", reason: "ended" });
    stop();
    store.subscribe("local", "c2", () => {});
    sock.receive({ sub: 2, type: "error", code: "not_found", message: "no such container" });
    vi.advanceTimersByTime(STATS_RETRY_MS * 3);
    expect(subs().map((f) => f.id)).toEqual(["c1", "c2"]);
  });
});
