import type { QueryClient, QueryKey } from "@tanstack/react-query";
import { keys } from "@/api/queries";
import type { DockerEvent } from "@/api/gen/types.gen";
import { isNoise, RECENT_EVENTS } from "./noise";

export { isNoise, RECENT_EVENTS };

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
    // Until the dashboard loads the feed, Docker's history covers this
    // event; creating the cache here would stop that load from happening.
    const feed = keys.recentEvents(env);
    if (this.qc.getQueryState(feed)) {
      this.qc.setQueryData<DockerEvent[]>(feed, (old = []) => [ev, ...old].slice(0, RECENT_EVENTS));
    }
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
