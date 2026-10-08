import type { ContainerActionName, ContainerSummary } from "@/api/gen/types.gen";
import { ACTIONS, actionsFor } from "./actions";

/** What can be done from the list to the containers chosen. */
export type ContainerQuick =
  | { type: "open"; tab: "logs" | "terminal" | "stats" }
  | { type: "action"; action: ContainerActionName; rows: ContainerSummary[] }
  | { type: "remove"; rows: ContainerSummary[] };

/**
 * The quick actions for `rows`, in menu order: where to look first (one
 * container only), then each Docker action with the containers whose state
 * it fits, Remove last. A viewer gets only what reads.
 */
export function containerQuick(rows: ContainerSummary[], canWrite: boolean): ContainerQuick[] {
  if (rows.length === 0) return [];
  const out: ContainerQuick[] = [];
  if (rows.length === 1) {
    const running = rows[0].state === "running";
    out.push({ type: "open", tab: "logs" });
    if (canWrite && running) out.push({ type: "open", tab: "terminal" });
    if (running) out.push({ type: "open", tab: "stats" });
  }
  if (!canWrite) return out;
  for (const a of ACTIONS) {
    const of = rows.filter((c) => actionsFor(c.state).includes(a.action));
    if (of.length > 0) out.push({ type: "action", action: a.action, rows: of });
  }
  out.push({ type: "remove", rows });
  return out;
}
