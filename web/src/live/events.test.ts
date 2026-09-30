import { QueryClient } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { EventApplier, keysForEvent, RECENT_EVENTS } from "./events";
import { keys } from "@/api/queries";
import type { DockerEvent } from "@/api/gen/types.gen";

const ev = (kind: string, action: string, attributes: Record<string, string> = {}): DockerEvent => ({
  kind,
  action,
  id: "abc",
  attributes,
  time: 1,
});

describe("keysForEvent", () => {
  it("container state changes refresh containers, compose projects and counts", () => {
    expect(keysForEvent("local", ev("container", "start"))).toEqual([
      keys.containers("local"),
      keys.composeProjects("local"),
      keys.info("local"),
    ]);
  });

  it("container create and destroy also refresh in-use flags and disk usage", () => {
    const got = keysForEvent("local", ev("container", "destroy"));
    expect(got).toContainEqual(keys.images("local"));
    expect(got).toContainEqual(keys.volumes("local"));
    expect(got).toContainEqual(keys.df("local"));
  });

  it("health status changes refresh the container list", () => {
    expect(keysForEvent("local", ev("container", "health_status: healthy"))).toContainEqual(keys.containers("local"));
  });

  it("exec and attach events refresh nothing", () => {
    expect(keysForEvent("local", ev("container", "exec_start: sh"))).toEqual([]);
    expect(keysForEvent("local", ev("container", "exec_die"))).toEqual([]);
    expect(keysForEvent("local", ev("container", "attach"))).toEqual([]);
  });

  it("images, volumes and networks refresh their lists", () => {
    expect(keysForEvent("local", ev("image", "pull"))).toContainEqual(keys.images("local"));
    expect(keysForEvent("local", ev("volume", "create"))).toContainEqual(keys.volumes("local"));
    expect(keysForEvent("local", ev("network", "connect", { container: "c1" }))).toEqual([
      keys.networks("local"),
      keys.container("local", "c1"),
    ]);
  });

  it("daemon events refresh the whole environment and unknown kinds nothing", () => {
    expect(keysForEvent("local", ev("daemon", "reload"))).toEqual([keys.env("local")]);
    expect(keysForEvent("local", ev("plugin", "enable"))).toEqual([]);
  });
});

describe("EventApplier", () => {
  let qc: QueryClient;
  beforeEach(() => {
    vi.useFakeTimers();
    qc = new QueryClient();
  });
  afterEach(() => vi.useRealTimers());

  it("invalidates each affected query once per batch", () => {
    const spy = vi.spyOn(qc, "invalidateQueries");
    const applier = new EventApplier(qc, 100);
    for (let i = 0; i < 20; i++) applier.apply("local", ev("container", "start"));
    expect(spy).not.toHaveBeenCalled();
    vi.advanceTimersByTime(100);
    expect(spy).toHaveBeenCalledTimes(3);
    expect(spy).toHaveBeenCalledWith({ queryKey: keys.containers("local") }, { cancelRefetch: false });
  });

  it("marks matching cached queries stale and leaves others alone", () => {
    qc.setQueryData(keys.containers("local"), []);
    qc.setQueryData(keys.networks("local"), []);
    const applier = new EventApplier(qc, 100);
    applier.apply("local", ev("container", "die"));
    applier.flush();
    expect(qc.getQueryState(keys.containers("local"))?.isInvalidated).toBe(true);
    expect(qc.getQueryState(keys.networks("local"))?.isInvalidated).toBe(false);
  });

  it("leaves exec noise out of the feed", () => {
    const applier = new EventApplier(qc, 100);
    applier.apply("local", ev("container", "exec_start: pg_isready"));
    applier.apply("local", ev("container", "start"));
    expect(qc.getQueryData<DockerEvent[]>(keys.recentEvents("local"))!.map((e) => e.action)).toEqual(["start"]);
    applier.dispose();
  });

  it("keeps the newest events first, capped", () => {
    const applier = new EventApplier(qc, 100);
    for (let i = 0; i < RECENT_EVENTS + 5; i++) applier.apply("local", { ...ev("container", "start"), time: i });
    const feed = qc.getQueryData<DockerEvent[]>(keys.recentEvents("local"))!;
    expect(feed).toHaveLength(RECENT_EVENTS);
    expect(feed[0].time).toBe(RECENT_EVENTS + 4);
    applier.dispose();
  });
});
