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
 * The feed after loading Docker's history (`past`, oldest first) while
 * live events (`live`, newest first) may already have arrived: newest
 * first, noise dropped, each event once, at most RECENT_EVENTS.
 */
export function mergeEvents(live: DockerEvent[], past: DockerEvent[]): DockerEvent[] {
  const seen = new Set<string>();
  const out: DockerEvent[] = [];
  for (const ev of [...live, ...[...past].reverse()]) {
    const k = `${ev.time}|${ev.kind}|${ev.id}|${ev.action}`;
    if (isNoise(ev) || seen.has(k)) continue;
    seen.add(k);
    out.push(ev);
  }
  return out.sort((a, b) => b.time - a.time).slice(0, RECENT_EVENTS);
}
