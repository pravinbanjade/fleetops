import type { KubeKind, KubeRow } from "@/api/gen/types.gen";
import { actsFor, type Act } from "./Act";
import { READ_ONLY } from "./kinds";

/**
 * What can be done from a list to the rows chosen: open one of a single
 * object's tabs, edit its YAML, or act on every row it fits.
 */
export type KubeQuick =
  | { type: "open"; tab: "logs" | "terminal" | "events" }
  | { type: "edit" }
  | { type: "act"; act: Act; rows: KubeRow[] };

/**
 * The quick actions for `rows` of `kind`, in the order a menu lists them:
 * where to look first (one object only), then its YAML, then the changes,
 * Delete last. A viewer gets only what reads.
 */
export function kubeQuick(kind: KubeKind, rows: KubeRow[], canWrite: boolean): KubeQuick[] {
  if (READ_ONLY.includes(kind) || rows.length === 0) return [];
  const out: KubeQuick[] = [];
  if (rows.length === 1) {
    if (kind === "pods") {
      out.push({ type: "open", tab: "logs" });
      // A shell needs a running container to start in.
      if (canWrite && rows[0].status === "Running") out.push({ type: "open", tab: "terminal" });
    }
    out.push({ type: "open", tab: "events" });
    if (canWrite) out.push({ type: "edit" });
  }
  if (canWrite) for (const { act, rows: of } of actsFor(kind, rows)) out.push({ type: "act", act, rows: of });
  return out;
}
