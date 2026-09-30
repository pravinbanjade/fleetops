import type { QueryClient, QueryKey } from "@tanstack/react-query";
import { keys } from "@/api/queries";
import type { DockerEvent } from "@/api/gen/types.gen";

/** How many events the dashboard feed keeps. */
export const RECENT_EVENTS = 50;

const QUIET = ["attach", "detach", "resize", "top", "export", "archive-path", "extract-to-dir"];

/**
 * Container events that change nothing the UI shows: exec (healthchecks
 * run one every few seconds per container), attach and file copies.
 */
export function isNoise(ev: DockerEvent): boolean {
  const action = ev.action.split(":")[0];
  return ev.kind === "container" && (action.startsWith("exec_") || QUIET.includes(action));
}

/**
 * The queries a Docker event makes stale; none for noise.
 */
export function keysForEvent(env: string, ev: DockerEvent): QueryKey[] {
  if (isNoise(ev)) return [];
  const action = ev.action.split(":")[0];
  switch (ev.kind) {
    case "container": {
      const out: QueryKey[] = [keys.containers(env), keys.composeProjects(env), keys.info(env)];
      if (["create", "destroy"].includes(action)) {
        out.push(keys.images(env), keys.volumes(env), keys.df(env));
      }
      return out;
    }
    case "image":
      return [keys.images(env), keys.info(env), keys.df(env)];
    case "volume":
      return [keys.volumes(env), keys.df(env)];
    case "network": {
      const out: QueryKey[] = [keys.networks(env)];
      const container = ev.attributes.container;
      if (container) out.push(keys.container(env, container));
      return out;
    }
    case "daemon":
      return [keys.env(env)];
    default:
      return [];
  }
}

/**
 * Applies Docker events to the query cache: it records each event other
 * than noise in the recent-events feed at once, and invalidates the affected queries in
 * batches, so a burst of events (a compose project starting) causes one
 * refetch per query rather than one per event.
 */
export class EventApplier {
  private readonly qc: QueryClient;
  private readonly delay: number;
  private readonly pending = new Map<string, QueryKey>();
  private timer: ReturnType<typeof setTimeout> | null = null;

  constructor(qc: QueryClient, delay = 100) {
    this.qc = qc;
    this.delay = delay;
  }

  apply(env: string, ev: DockerEvent) {
    if (isNoise(ev)) return;
    this.qc.setQueryData<DockerEvent[]>(keys.recentEvents(env), (old = []) => [ev, ...old].slice(0, RECENT_EVENTS));
    for (const key of keysForEvent(env, ev)) this.pending.set(JSON.stringify(key), key);
    if (this.pending.size > 0 && !this.timer) {
      this.timer = setTimeout(() => this.flush(), this.delay);
    }
  }

  flush() {
    if (this.timer) clearTimeout(this.timer);
    this.timer = null;
    const batch = [...this.pending.values()];
    this.pending.clear();
    // A refetch already in flight may have been answered before these
    // events happened, so it is cancelled and started again.
    for (const queryKey of batch) {
      void this.qc.invalidateQueries({ queryKey });
    }
  }

  dispose() {
    if (this.timer) clearTimeout(this.timer);
    this.timer = null;
    this.pending.clear();
  }
}
