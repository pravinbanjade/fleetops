import type { DockerEvent, KubeRow } from "@/api/gen/types.gen";
import { isNoise } from "@/live/noise";

/** One event on the tape, newest first. `target` is the list to show it on and the row to select there. */
export type TapeItem = {
  key: string;
  at: number;
  text: string;
  failing: boolean;
  target?: { to: string; params?: Record<string, string>; select?: string };
};

/** How many items the tape keeps. */
export const TAPE_MAX = 50;

/** Newest first, at most `max`. */
export function capTape(items: TapeItem[], max = TAPE_MAX): TapeItem[] {
  return [...items].sort((a, b) => b.at - a.at).slice(0, max);
}

const LISTS: Record<string, string> = {
  container: "/envs/$env/containers",
  image: "/envs/$env/images",
  volume: "/envs/$env/volumes",
  network: "/envs/$env/networks",
  service: "/envs/$env/swarm/services",
  node: "/envs/$env/swarm/nodes",
};

/** What a Docker event says on the tape; null for the noise the feeds drop. */
export function dockerTapeItem(ev: DockerEvent): TapeItem | null {
  if (isNoise(ev)) return null;
  const action = ev.action.split(":")[0];
  // Swarm and daemon events can arrive with no attributes at all.
  const attrs = ev.attributes ?? {};
  const exit = attrs.exitCode;
  const failing =
    (action === "die" && !!exit && exit !== "0") || action === "oom" || action === "kill" || ev.action === "health_status: unhealthy";
  const what = ev.name ?? attrs.name ?? ev.id.slice(0, 12);
  const text = `${what} ${ev.action.startsWith("health_status") ? ev.action.replace("health_status: ", "") : action}${action === "die" && exit ? ` (${exit})` : ""}`;
  const to = LISTS[ev.kind];
  return { key: `${ev.time}|${ev.kind}|${ev.id}|${ev.action}`, at: ev.time, text, failing, target: to ? { to, select: ev.id } : undefined };
}

/** What a Kubernetes event says on the tape: its object and reason; a warning is failing. */
export function kubeTapeItem(row: KubeRow): TapeItem {
  const c = row.columns;
  const at = Number(c.last_seen ?? row.created) || row.created;
  return {
    key: `${row.namespace ?? ""}/${row.name}/${at}`,
    at,
    text: `${c.object_name ?? row.name} ${c.reason ?? row.status}`,
    failing: row.status === "Warning",
    target: { to: "/envs/$env/k8s/$kind", params: { kind: "events" } },
  };
}
