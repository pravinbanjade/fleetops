import { describe, expect, it } from "vitest";
import type { DockerEvent, NodeSummary } from "@/api/gen/types.gen";
import { container } from "@/test/docker";
import { dockerStatus, swarmStatus } from "./findings";

const running = (i: number) => container({ id: `c${i}`, name: `app-${i}`, state: "running", status: "Up 6 days" });
const die = (time: number, id = "wb", name = "worker-billing"): DockerEvent => ({ action: "die", attributes: { exitCode: "137" }, id, kind: "container", name, time });
const NOW = 10_000;

describe("dockerStatus", () => {
  it("names a restart loop after the running count", () => {
    const list = [...Array.from({ length: 7 }, (_, i) => running(i)), container({ id: "wb", name: "worker-billing", state: "restarting", status: "Restarting (137) 2 seconds ago" })];
    expect(dockerStatus(list, [die(NOW - 30), die(NOW - 200), die(NOW - 500)], NOW)).toBe("7 of 8 running · worker-billing has restarted 3 times in 10 minutes");
  });

  it("is the count alone when nothing needs attention", () => {
    expect(dockerStatus(Array.from({ length: 8 }, (_, i) => running(i)), [], NOW)).toBe("8 of 8 running");
  });

  it("does not call two deaths 11 minutes apart a loop", () => {
    const list = Array.from({ length: 2 }, (_, i) => running(i));
    expect(dockerStatus(list, [die(NOW - 10), die(NOW - 670)], NOW)).toBe("2 of 2 running");
  });

  it("counts containers that exited with errors when there is no loop", () => {
    const list = [running(1), container({ id: "x", name: "job", state: "exited", status: "Exited (1) 3 minutes ago" })];
    expect(dockerStatus(list, [], NOW)).toBe("1 of 2 running · 1 exited with errors");
  });
});

describe("swarmStatus", () => {
  const node = (state: string): NodeSummary => ({ availability: "active", hostname: "h", id: state + Math.random(), leader: false, role: "worker", state });
  it("counts ready nodes and names the down ones", () => {
    expect(swarmStatus([node("ready"), node("ready"), node("down")])).toBe("2 of 3 nodes ready · 1 node down");
    expect(swarmStatus([node("ready")])).toBe("1 of 1 node ready");
  });
});
