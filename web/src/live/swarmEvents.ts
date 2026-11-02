import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useId, useSyncExternalStore } from "react";
import type { DockerEvent } from "@/api/gen/types.gen";
import { swarmKeys } from "@/api/queries";
import { useSubscription } from "./LiveProvider";

/** How many swarm events the overview keeps. */
export const SWARM_EVENTS_KEPT = 20;

/** The queries a swarm event makes stale. */
function staleFor(env: string, ev: DockerEvent) {
  switch (ev.kind) {
    case "service":
      return [swarmKeys.services(env)];
    case "node":
      return [swarmKeys.nodes(env), ["env", env, "swarm", "agents"]];
    case "secret":
      return [swarmKeys.secrets(env)];
    case "config":
      return [swarmKeys.configs(env)];
    case "network":
      return [swarmKeys.networks(env)];
    default:
      return [];
  }
}

/*
 * The tape and the page on screen both want a swarm's events. One of them
 * holds the subscription and the cache is shared, so the server streams each
 * event once; when the holder unmounts the next in line takes over.
 */
const holders = new Map<string, string[]>();
const listeners = new Set<() => void>();
const notify = () => listeners.forEach((l) => l());
const listen = (l: () => void) => {
  listeners.add(l);
  return () => listeners.delete(l);
};

function useHolds(env: string): boolean {
  const me = useId();
  useEffect(() => {
    holders.set(env, [...(holders.get(env) ?? []), me]);
    notify();
    return () => {
      const rest = (holders.get(env) ?? []).filter((h) => h !== me);
      if (rest.length) holders.set(env, rest);
      else holders.delete(env);
      notify();
    };
  }, [env, me]);
  return useSyncExternalStore(listen, () => holders.get(env)?.[0] === me);
}

/** Keeps a swarm's cluster lists current from its events, and remembers the newest. */
export function useSwarmEvents(env: string) {
  const qc = useQueryClient();
  const holds = useHolds(env);
  useSubscription(
    holds ? { topic: "events", env } : null,
    {
      onData: (d) => {
        const ev = d as DockerEvent;
        for (const key of staleFor(env, ev)) void qc.invalidateQueries({ queryKey: key });
        // The shell's tape and a page may both listen: keep each event once.
        const same = (e: DockerEvent) => e.time === ev.time && e.id === ev.id && e.action === ev.action;
        qc.setQueryData<DockerEvent[]>(swarmKeys.events(env), (old = []) => (old.some(same) ? old : [ev, ...old].slice(0, SWARM_EVENTS_KEPT)));
      },
      onEnd: () => {},
      onError: () => {},
    },
  );
}
