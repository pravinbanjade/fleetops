import type { ContainerSummary } from "@/api/gen/types.gen";

/** Docker's `(unhealthy)` marker in a running container's status. */
const UNHEALTHY = /\(unhealthy\)/;
/** The exit code in an exited container's status: "Exited (137) 2 minutes ago". */
const EXITED = /^Exited \((-?\d+)\)/;

/**
 * The state a container is shown under: its Docker state, except that a
 * running container whose healthcheck fails is "unhealthy".
 */
export function shownState(c: ContainerSummary): string {
  return c.state === "running" && UNHEALTHY.test(c.status) ? "unhealthy" : c.state;
}

/**
 * Whether a container needs someone to look at it: it is unhealthy,
 * restarting, dead, or exited with a code other than 0. A job that ran to
 * completion (exit 0) and a container that was never started do not.
 */
export function needsAttention(c: ContainerSummary): boolean {
  const state = shownState(c);
  if (state === "unhealthy" || state === "restarting" || state === "dead") return true;
  if (state !== "exited") return false;
  const code = EXITED.exec(c.status)?.[1];
  return code !== undefined && code !== "0";
}

/** The order states are listed in: problems first, then the rest. */
const ORDER = ["unhealthy", "restarting", "dead", "running", "paused", "exited", "created", "removing"];

/** How many containers are in each shown state, problems first. */
export function stateCounts(list: ContainerSummary[]): [state: string, count: number][] {
  const counts = new Map<string, number>();
  for (const c of list) {
    const s = shownState(c);
    counts.set(s, (counts.get(s) ?? 0) + 1);
  }
  const rank = (s: string) => (ORDER.includes(s) ? ORDER.indexOf(s) : ORDER.length);
  return [...counts].sort(([a], [b]) => rank(a) - rank(b) || a.localeCompare(b));
}

/** The containers that need attention, most urgent first. */
export function attentionList(list: ContainerSummary[]): ContainerSummary[] {
  const rank = (c: ContainerSummary) => ORDER.indexOf(shownState(c));
  return list.filter(needsAttention).sort((a, b) => rank(a) - rank(b) || a.name.localeCompare(b.name));
}
