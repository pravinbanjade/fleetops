import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { defaultBackoff, WsClient, wsUrl } from "./ws";
import { FakeWebSocket } from "@/test/fake-ws";

function client(extra: Partial<ConstructorParameters<typeof WsClient>[0]> = {}) {
  const ws = new WsClient({
    url: "ws://test/api/v1/ws",
    WebSocketImpl: FakeWebSocket as unknown as typeof WebSocket,
    backoff: () => 100,
    ...extra,
  });
  ws.start();
  return ws;
}

describe("WsClient", () => {
  beforeEach(() => {
    FakeWebSocket.instances = [];
    vi.useFakeTimers();
  });
  afterEach(() => vi.useRealTimers());

  it("queues subscriptions until the socket opens", () => {
    const ws = client();
    ws.subscribe({ topic: "events", env: "local" }, { onData: () => {} });
    const sock = FakeWebSocket.latest();
    expect(sock.sent).toEqual([]);
    sock.open();
    expect(sock.frames()).toEqual([{ op: "sub", sub: 1, topic: "events", env: "local" }]);
  });

  it("routes data, gap and end to the subscription's handlers", () => {
    const ws = client();
    const sock = FakeWebSocket.latest();
    sock.open();
    const onData = vi.fn();
    const onGap = vi.fn();
    const onEnd = vi.fn();
    const other = vi.fn();
    ws.subscribe({ topic: "logs", env: "local", id: "c1" }, { onData, onGap, onEnd });
    ws.subscribe({ topic: "events", env: "local" }, { onData: other });
    sock.receive({ sub: 1, type: "data", data: [{ line: "hi" }] });
    sock.receive({ sub: 1, type: "gap", dropped: 7 });
    sock.receive({ sub: 1, type: "end", reason: "eof" });
    sock.receive({ sub: 1, type: "data", data: "after end" });
    expect(onData).toHaveBeenCalledExactlyOnceWith([{ line: "hi" }]);
    expect(onGap).toHaveBeenCalledWith(7);
    expect(onEnd).toHaveBeenCalledWith({ sub: 1, type: "end", reason: "eof" });
    expect(other).not.toHaveBeenCalled();
  });

  it("drops a subscription the server refused", () => {
    const ws = client();
    const sock = FakeWebSocket.latest();
    sock.open();
    const onError = vi.fn();
    ws.subscribe({ topic: "stats", env: "nope", id: "c1" }, { onData: () => {}, onError });
    sock.receive({ sub: 1, type: "error", code: "not_found", message: "no env" });
    expect(onError).toHaveBeenCalledOnce();
    sock.drop();
    vi.advanceTimersByTime(100);
    FakeWebSocket.latest().open();
    expect(FakeWebSocket.latest().frames()).toEqual([]);
  });

  it("sends unsub and ignores later frames", () => {
    const ws = client();
    const sock = FakeWebSocket.latest();
    sock.open();
    const onData = vi.fn();
    const { unsubscribe } = ws.subscribe({ topic: "events", env: "local" }, { onData });
    unsubscribe();
    unsubscribe();
    sock.receive({ sub: 1, type: "data", data: {} });
    expect(onData).not.toHaveBeenCalled();
    expect(sock.frames().at(-1)).toEqual({ op: "unsub", sub: 1 });
    expect(sock.frames().filter((f) => f.op === "unsub")).toHaveLength(1);
  });

  it("reconnects with backoff and resubscribes every active subscription", () => {
    const delays: number[] = [];
    const onReconnect = vi.fn();
    const ws = client({ backoff: (n) => (delays.push(n), 1000 * (n + 1)), onReconnect });
    FakeWebSocket.latest().open();
    const onResubscribe = vi.fn();
    ws.subscribe({ topic: "events", env: "local" }, { onData: () => {}, onResubscribe });
    ws.subscribe({ topic: "stats", env: "local", id: "c1" }, { onData: () => {} });

    FakeWebSocket.latest().drop();
    vi.advanceTimersByTime(999);
    expect(FakeWebSocket.instances).toHaveLength(1);
    vi.advanceTimersByTime(1);
    expect(FakeWebSocket.instances).toHaveLength(2);
    FakeWebSocket.latest().drop(); // the attempt failed
    vi.advanceTimersByTime(2000);
    const sock = FakeWebSocket.latest();
    expect(FakeWebSocket.instances).toHaveLength(3);
    sock.open();

    expect(delays).toEqual([0, 1]);
    expect(sock.frames()).toEqual([
      { op: "sub", sub: 1, topic: "events", env: "local" },
      { op: "sub", sub: 2, topic: "stats", env: "local", id: "c1" },
    ]);
    expect(onResubscribe).toHaveBeenCalledOnce();
    expect(onReconnect).toHaveBeenCalledOnce();
  });

  it("reports reconnecting from a drop until a socket opens again", () => {
    const states: string[] = [];
    client({ onState: (s) => states.push(s) });
    FakeWebSocket.latest().open();
    FakeWebSocket.latest().drop();
    vi.advanceTimersByTime(100);
    FakeWebSocket.latest().drop();
    vi.advanceTimersByTime(100);
    FakeWebSocket.latest().open();
    expect(states).toEqual(["connecting", "open", "reconnecting", "open"]);
  });

  it("reports reconnecting when the first connection fails", () => {
    const states: string[] = [];
    client({ onState: (s) => states.push(s) });
    FakeWebSocket.latest().drop();
    expect(states).toEqual(["connecting", "reconnecting"]);
  });

  it("does not reconnect after close(), and start() opens it again with the same subscriptions", () => {
    const ws = client();
    FakeWebSocket.latest().open();
    ws.subscribe({ topic: "events", env: "local" }, { onData: () => {} });
    ws.close();
    vi.advanceTimersByTime(60_000);
    expect(FakeWebSocket.instances).toHaveLength(1);
    ws.start();
    FakeWebSocket.latest().open();
    expect(FakeWebSocket.latest().frames()).toEqual([{ op: "sub", sub: 1, topic: "events", env: "local" }]);
  });

  it("does not connect until started", () => {
    new WsClient({ url: "ws://test/api/v1/ws", WebSocketImpl: FakeWebSocket as unknown as typeof WebSocket });
    expect(FakeWebSocket.instances).toHaveLength(0);
  });

  it("reports env_status frames", () => {
    const onEnvStatus = vi.fn();
    client({ onEnvStatus });
    const sock = FakeWebSocket.latest();
    sock.open();
    sock.receive({ type: "env_status", env: "local", status: { state: "down", reason: "gone" } });
    expect(onEnvStatus).toHaveBeenCalledWith("local", { state: "down", reason: "gone" });
  });

  it("frames binary exec input and routes binary output by sub id", () => {
    const ws = client();
    const sock = FakeWebSocket.latest();
    sock.open();
    const onBinary = vi.fn();
    const { sub } = ws.subscribe({ topic: "exec", env: "local", id: "c1" }, { onData: () => {}, onBinary });
    ws.sendBinary(sub, new Uint8Array([104, 105]));
    expect(sock.sent.at(-1)).toEqual(new Uint8Array([0, 0, 0, 1, 104, 105]));
    const out = new Uint8Array([0, 0, 0, 1, 111, 107]).buffer;
    sock.receive(out);
    expect(onBinary).toHaveBeenCalledWith(new Uint8Array([111, 107]));
  });

  it("ignores frames that are not JSON or name no subscription", () => {
    client();
    const sock = FakeWebSocket.latest();
    sock.open();
    expect(() => sock.onmessage?.({ data: "not json" })).not.toThrow();
    expect(() => sock.receive({ type: "error", code: "invalid", message: "bad frame", sub: null })).not.toThrow();
  });
});

describe("wsUrl", () => {
  it("uses wss on https", () => {
    expect(wsUrl({ protocol: "https:", host: "fleet.lan:9443" } as Location)).toBe("wss://fleet.lan:9443/api/v1/ws");
    expect(wsUrl({ protocol: "http:", host: "localhost:5173" } as Location)).toBe("ws://localhost:5173/api/v1/ws");
  });
});

describe("defaultBackoff", () => {
  it("doubles from 500 ms up to 30 s", () => {
    expect([0, 1, 2, 5, 6, 20].map(defaultBackoff)).toEqual([500, 1000, 2000, 16000, 30000, 30000]);
  });
});
