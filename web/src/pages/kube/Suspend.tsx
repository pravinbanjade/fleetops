import { useQueryClient } from "@tanstack/react-query";
import { resumeKube, suspendKube } from "@/api/gen/sdk.gen";
import type { KubeKind, KubeRow } from "@/api/gen/types.gen";
import { keys } from "@/api/queries";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { KIND_LABEL } from "./kinds";

/** One object to suspend or resume. */
export type SuspendTarget = { kind: KubeKind; ns: string; name: string; row?: KubeRow };

/** FleetOps' own suspend: Deployments and StatefulSets at zero with their count kept, or a CronJob's `spec.suspend`. */
export const isSuspended = (row: KubeRow | undefined) => row?.health === "off" && row.status === "Suspended";

/** Replicas wanted, from a `ready` column like "2/3". */
export const replicasOf = (row?: KubeRow) => {
  const n = Number(row?.columns.ready?.split("/")[1]);
  return Number.isFinite(n) ? n : undefined;
};

function what(targets: SuspendTarget[]) {
  if (targets.length !== 1) return `${targets.length} workloads`;
  const t = targets[0];
  return `${KIND_LABEL[t.kind].replace(/s$/, "")} ${t.name}`;
}

/** What suspending or resuming will do, said plainly before it happens. */
function consequence(targets: SuspendTarget[], on: boolean) {
  const crons = targets.every((t) => t.kind === "cronjobs");
  const scaled = targets.some((t) => t.kind !== "cronjobs");
  if (on) {
    const one = targets.length === 1 ? replicasOf(targets[0].row) : undefined;
    const parts = [
      crons
        ? "No new Jobs start until you resume it; Jobs already running finish."
        : `Scales ${targets.length === 1 ? "it" : "each"} to 0 replicas and remembers ${one !== undefined ? `its ${one}` : "the count"}, so Resume brings back exactly that many. Pods stop; the object, its config and its history stay.`,
    ];
    if (scaled && !crons) parts.push("If a HorizontalPodAutoscaler manages it, the autoscaler may scale it back up.");
    return parts.join(" ");
  }
  if (crons) return "Its schedule runs again from the next time it is due.";
  const kept = targets.length === 1 ? targets[0].row?.columns.suspended_replicas : undefined;
  return `Scales ${targets.length === 1 ? "it" : "each"} back to ${kept ? `${kept} ${kept === "1" ? "replica" : "replicas"}` : "the replicas it had"}.`;
}

/**
 * Confirms, then suspends or resumes every target. Stops at the first
 * failure and says which one, so a half-done bulk action is never silent.
 */
export function SuspendDialog({
  env,
  targets,
  on,
  onClose,
  onDone,
}: {
  env: string;
  targets: SuspendTarget[];
  on: boolean;
  onClose: () => void;
  onDone?: () => void;
}) {
  const qc = useQueryClient();
  const run = async () => {
    for (const t of targets) {
      const path = { env, kind: t.kind, ns: t.ns, name: t.name };
      try {
        await (on ? suspendKube : resumeKube)({ path, throwOnError: true });
      } catch (e) {
        if (targets.length === 1) throw e;
        throw new Error(`${t.name}: ${e instanceof Error ? e.message : String(e)}`, { cause: e });
      }
    }
    // Lists follow their watches; this refreshes object pages and counts.
    await qc.invalidateQueries({ queryKey: keys.env(env) });
    onDone?.();
  };
  return (
    <ConfirmDialog
      open
      onOpenChange={(o) => !o && onClose()}
      title={`${on ? "Suspend" : "Resume"} ${what(targets)}?`}
      description={consequence(targets, on)}
      confirmLabel={on ? "Suspend" : "Resume"}
      // Reversible either way: the cord, not the destructive red.
      destructive={false}
      onConfirm={run}
    />
  );
}
