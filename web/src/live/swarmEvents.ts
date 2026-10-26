import { useQueryClient } from "@tanstack/react-query";
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

/** Keeps a swarm's cluster lists current from its events, and remembers the newest. */
export function useSwarmEvents(env: string) {
  const qc = useQueryClient();
  useSubscription(
    { topic: "events", env },
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
