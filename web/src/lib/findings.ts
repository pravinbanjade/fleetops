import type { ContainerSummary, DockerEvent } from "@/api/gen/types.gen";
import { needsAttention, shownState } from "./health";

/** How far back a restart loop is looked for, in seconds. */
const LOOP_WINDOW = 600;

const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

/**
 * A Docker host's status line: how many containers run, then at most one
 * finding, the first that applies: a restart loop (two or more deaths of
 * one container in ten minutes), unhealthy containers, containers that
 * exited with an error.
 */
export function dockerStatus(containers: ContainerSummary[], events: DockerEvent[], now: number): string {
  const count = `${containers.filter((c) => c.state === "running").length} of ${containers.length} running`;
  const deaths = new Map<string, { name: string; n: number }>();
  for (const e of events) {
    if (e.kind !== "container" || e.action !== "die" || now - e.time > LOOP_WINDOW) continue;
    const d = deaths.get(e.id) ?? { name: e.name ?? e.id.slice(0, 12), n: 0 };
    deaths.set(e.id, { ...d, n: d.n + 1 });
  }
  const loop = [...deaths.values()].sort((a, b) => b.n - a.n)[0];
  if (loop && loop.n >= 2) return `${count} · ${loop.name} has restarted ${loop.n} times in 10 minutes`;
  const unhealthy = containers.filter((c) => shownState(c) === "unhealthy").length;
  if (unhealthy) return `${count} · ${unhealthy} unhealthy`;
  const failed = containers.filter((c) => c.state === "exited" && needsAttention(c)).length;
  if (failed) return `${count} · ${failed} exited with errors`;
  return count;
}

/** A swarm's status line: ready nodes, and how many are down. */
export function swarmStatus(nodes: { state?: string | null }[]): string {
  const ready = nodes.filter((n) => n.state === "ready").length;
  const line = `${ready} of ${plural(nodes.length, "node")} ready`;
  const down = nodes.filter((n) => n.state === "down").length;
  return down ? `${line} · ${plural(down, "node")} down` : line;
}
