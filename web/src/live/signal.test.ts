import { describe, expect, it } from "vitest";
import { FakeWebSocket } from "@/test/fake-ws";
import { subscribeSignal } from "./signal";
import { WsClient } from "./ws";

function setup() {
  const ws = new WsClient({ url: "ws://x/api/v1/ws", backoff: () => 0 });
  ws.start();
  const sock = FakeWebSocket.latest();
  sock.open();
  return { ws, sock };
}

describe("subscribeSignal", () => {
  it("subscribes to the key and calls back once per ping", () => {
    const { ws, sock } = setup();
    let n = 0;
    subscribeSignal(ws, "security", () => n++);
    const subs = sock.frames().filter((f) => f.op === "sub");
    expect(subs).toHaveLength(1);
    expect(subs[0]).toMatchObject({ topic: "signal", id: "security" });
    sock.receive({ sub: subs[0].sub, type: "data", data: { key: "security" } });
    sock.receive({ sub: subs[0].sub, type: "data", data: { key: "security" } });
    expect(n).toBe(2);
  });

  it("stops calling back after unsubscribe", () => {
    const { ws, sock } = setup();
    let n = 0;
    const stop = subscribeSignal(ws, "security", () => n++);
    const sub = sock.frames()[0].sub;
    stop();
    expect(sock.frames().some((f) => f.op === "unsub" && f.sub === sub)).toBe(true);
    sock.receive({ sub, type: "data", data: { key: "security" } });
    expect(n).toBe(0);
  });
});
