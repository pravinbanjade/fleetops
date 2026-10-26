import { describe, expect, it } from "vitest";
import type { DockerEvent, KubeRow } from "@/api/gen/types.gen";
import { capTape, dockerTapeItem, kubeTapeItem, type TapeItem } from "./tapeItems";

const ev = (over: Partial<DockerEvent>): DockerEvent => ({ action: "start", attributes: {}, id: "c1", kind: "container", name: "web", time: 100, ...over });

describe("tape", () => {
  it("keeps only the newest 50 items", () => {
    const items: TapeItem[] = Array.from({ length: 80 }, (_, i) => ({ key: `k${i}`, at: i, text: `e${i}`, failing: false }));
    const kept = capTape(items);
    expect(kept).toHaveLength(50);
    expect(kept[0].at).toBe(79);
    expect(kept.at(-1)!.at).toBe(30);
  });

  it("marks a container that died with an error as failing, with its exit code", () => {
    const item = dockerTapeItem(ev({ action: "die", name: "worker-billing", attributes: { exitCode: "137" } }))!;
    expect(item.failing).toBe(true);
    expect(item.text).toContain("worker-billing");
    expect(item.text).toContain("(137)");
    expect(item.target).toEqual({ to: "/envs/$env/containers", select: "c1" });
  });

  it("does not mark a clean exit as failing", () => {
    expect(dockerTapeItem(ev({ action: "die", attributes: { exitCode: "0" } }))!.failing).toBe(false);
  });

  it("marks an OOM kill and an unhealthy check as failing", () => {
    expect(dockerTapeItem(ev({ action: "oom" }))!.failing).toBe(true);
    expect(dockerTapeItem(ev({ action: "health_status: unhealthy" }))!.failing).toBe(true);
  });

  it("drops the noise the feeds already drop", () => {
    expect(dockerTapeItem(ev({ action: "exec_start: sh -c true" }))).toBeNull();
    expect(dockerTapeItem(ev({ action: "attach" }))).toBeNull();
  });

  it("marks a Kubernetes warning as failing and names its object", () => {
    const row = {
      name: "ev1",
      namespace: "prod",
      status: "Warning",
      created: 5,
      health: "warn",
      columns: { reason: "BackOff", object_kind: "Pod", object_name: "api-7d9", last_seen: "200" },
    } as unknown as KubeRow;
    const item = kubeTapeItem(row);
    expect(item.failing).toBe(true);
    expect(item.text).toBe("api-7d9 BackOff");
    expect(item.at).toBe(200);
  });

  it("survives an event that carries no attributes", () => {
    const bare = { action: "update", id: "s1", kind: "service", name: "blog_web", time: 1 } as unknown as DockerEvent;
    expect(dockerTapeItem(bare)!.text).toBe("blog_web update");
  });
});
