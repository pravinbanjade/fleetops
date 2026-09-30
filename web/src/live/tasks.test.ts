import { describe, expect, it } from "vitest";
import { FakeWebSocket } from "@/test/fake-ws";
import { TaskTracker, type TrackedTask } from "./tasks";
import { WsClient } from "./ws";

function setup() {
  const ws = new WsClient({ url: "ws://x/api/v1/ws", backoff: () => 0 });
  ws.start();
  const sock = FakeWebSocket.latest();
  sock.open();
  const done: TrackedTask[] = [];
  const tracker = new TaskTracker(ws, (t) => done.push(t));
  return { ws, sock, tracker, done };
}

describe("TaskTracker", () => {
  it("collects progress and reports success once", () => {
    const { sock, tracker, done } = setup();
    tracker.track("t1", "local", "Pull nginx");
    tracker.track("t1", "local", "Pull nginx");
    const subs = sock.frames().filter((f) => f.op === "sub");
    expect(subs).toHaveLength(1);
    expect(subs[0]).toMatchObject({ topic: "task", id: "t1" });
    const sub = subs[0].sub;
    sock.receive({ sub, type: "data", data: { status: "Downloading" } });
    expect(tracker.get("t1")?.messages).toEqual([{ status: "Downloading" }]);
    sock.receive({ sub, type: "end", reason: "succeeded", task: { status: "succeeded" } });
    expect(tracker.get("t1")?.status).toBe("succeeded");
    expect(done.map((t) => t.status)).toEqual(["succeeded"]);
  });

  it("reports the task's error when it fails", () => {
    const { sock, tracker, done } = setup();
    tracker.track("t1", "local", "Pull nope");
    const sub = sock.frames()[0].sub;
    sock.receive({ sub, type: "end", reason: "failed", task: { status: "failed", error: "manifest unknown" } });
    expect(done[0]).toMatchObject({ status: "failed", error: "manifest unknown" });
  });

  it("fails a task the server will not follow", () => {
    const { sock, tracker, done } = setup();
    tracker.track("gone", "local", "Build app");
    const sub = sock.frames()[0].sub;
    sock.receive({ sub, type: "error", code: "not_found", message: "task 'gone' not found" });
    expect(done[0]).toMatchObject({ status: "failed", error: "task 'gone' not found" });
  });

  it("starts over when the server resends the backlog after a reconnect", () => {
    const { sock, tracker } = setup();
    tracker.track("t1", "local", "Pull nginx");
    sock.receive({ sub: sock.frames()[0].sub, type: "data", data: { status: "a" } });
    sock.drop();
    const next = FakeWebSocket.latest();
    return new Promise<void>((resolve) =>
      setTimeout(() => {
        next.open();
        expect(tracker.get("t1")?.messages).toEqual([]);
        next.receive({ sub: next.frames()[0].sub, type: "data", data: { status: "a" } });
        expect(tracker.get("t1")?.messages).toEqual([{ status: "a" }]);
        resolve();
      }, 5),
    );
  });
});
