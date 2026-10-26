import { useQueryClient } from "@tanstack/react-query";
import { Ban, CircleCheck, Pause, Play, RotateCw, SlidersHorizontal, Trash2, Zap, type LucideIcon } from "lucide-react";
import { useRef, useState } from "react";
import { cordonKube, deleteKube, restartKube, scaleKube, triggerKube, uncordonKube } from "@/api/gen/sdk.gen";
import type { KubeKind, KubeRow } from "@/api/gen/types.gen";
import { keys } from "@/api/queries";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { FormDialog } from "@/pages/admin/FormDialog";
import { KIND_LABEL, READ_ONLY, RESTARTABLE, SCALABLE, SUSPENDABLE } from "./kinds";
import { SuspendDialog, isSuspended, replicasOf, type SuspendTarget } from "./Suspend";

/** What can be done to one object or a selection of them. */
export type Act = "restart" | "scale" | "suspend" | "resume" | "trigger" | "cordon" | "uncordon" | "delete";
export type ActTarget = SuspendTarget;

const ACTS: Record<Act, { label: string; icon: LucideIcon }> = {
  restart: { label: "Restart", icon: RotateCw },
  scale: { label: "Scale", icon: SlidersHorizontal },
  suspend: { label: "Suspend", icon: Pause },
  resume: { label: "Resume", icon: Play },
  trigger: { label: "Run now", icon: Zap },
  cordon: { label: "Cordon", icon: Ban },
  uncordon: { label: "Uncordon", icon: CircleCheck },
  delete: { label: "Delete", icon: Trash2 },
};

export const actLabel = (act: Act) => ACTS[act].label;
export const actIcon = (act: Act) => ACTS[act].icon;

/** `kubectl cordon`'s mark: the node takes no new pods. */
export const isCordoned = (row: KubeRow | undefined) => row?.columns.unschedulable === "true";

/**
 * The actions that apply to `rows` of `kind`, each with the rows it fits:
 * a selection of running and suspended Deployments offers both Suspend and
 * Resume, each for its own. Delete comes last.
 */
export function actsFor(kind: KubeKind, rows: KubeRow[]): { act: Act; rows: KubeRow[] }[] {
  if (READ_ONLY.includes(kind)) return [];
  const live = rows.filter((r) => !isSuspended(r));
  const out: { act: Act; rows: KubeRow[] }[] = [];
  const add = (act: Act, of: KubeRow[]) => of.length > 0 && out.push({ act, rows: of });
  if (RESTARTABLE.includes(kind)) add("restart", live);
  if (SCALABLE.includes(kind)) add("scale", live);
  if (kind === "cronjobs") add("trigger", rows);
  if (SUSPENDABLE.includes(kind)) {
    add("suspend", live);
    add("resume", rows.filter(isSuspended));
  }
  if (kind === "nodes") {
    add("cordon", rows.filter((r) => !isCordoned(r)));
    add("uncordon", rows.filter(isCordoned));
  }
  add("delete", rows);
  return out;
}

const one = (kind: KubeKind) => KIND_LABEL[kind].replace(/s$/, "");

function what(targets: ActTarget[]) {
  if (targets.length !== 1) {
    const kinds = new Set(targets.map((t) => t.kind));
    return kinds.size === 1 ? `${targets.length} ${KIND_LABEL[targets[0].kind]}` : `${targets.length} objects`;
  }
  return `${one(targets[0].kind)} ${targets[0].name}`;
}

/** What the action will do, said plainly before it happens. */
function consequence(act: Act, targets: ActTarget[]) {
  const many = targets.length > 1;
  const kind = targets[0]?.kind;
  switch (act) {
    case "restart":
      return `${many ? "Their" : "Its"} pods are replaced one by one, as kubectl rollout restart does.`;
    case "trigger":
      return `Starts a Job from ${many ? "each" : "its"} template now, as kubectl create job --from=cronjob does. The schedule is unchanged.`;
    case "cordon":
      return `No new pods are scheduled on ${many ? "them" : "it"}. Pods already there keep running; delete them to move them elsewhere.`;
    case "uncordon":
      return `${many ? "They take" : "It takes"} new pods again.`;
    case "delete":
      if (kind === "namespaces") return "Everything in the namespace is deleted with it. This cannot be undone.";
      if (kind === "pods") return `This cannot be undone. A pod a Deployment, StatefulSet, DaemonSet or Job owns is replaced by a new one.`;
      if (kind === "nodes") return "The node leaves the cluster's API; its kubelet may register it again. This cannot be undone.";
      return "This cannot be undone.";
    default:
      return undefined;
  }
}

async function runOne(env: string, act: Act, t: ActTarget, replicas: number): Promise<string | undefined> {
  const path = { env, kind: t.kind, ns: t.ns, name: t.name };
  switch (act) {
    case "restart":
      await restartKube({ path, throwOnError: true });
      return;
    case "scale":
      await scaleKube({ path, body: { replicas }, throwOnError: true });
      return;
    case "trigger":
      return (await triggerKube({ path, throwOnError: true })).data.job;
    case "cordon":
      await cordonKube({ path, throwOnError: true });
      return;
    case "uncordon":
      await uncordonKube({ path, throwOnError: true });
      return;
    case "delete":
      await deleteKube({ path, throwOnError: true });
      return;
    default:
      throw new Error(`${act} runs through its own dialog`);
  }
}

/**
 * Confirms, then runs `act` on every target in turn. On a failure the
 * dialog stays open saying how far it got and which one failed; trying
 * again picks up from there rather than repeating what already happened.
 * `onDone` gets what each target returned (Run now: the Job's name).
 */
export function ActDialog({
  env,
  act,
  targets,
  onClose,
  onDone,
}: {
  env: string;
  act: Act;
  targets: ActTarget[];
  onClose: () => void;
  onDone?: (results: (string | undefined)[]) => void;
}) {
  const qc = useQueryClient();
  const done = useRef(new Map<string, string | undefined>());
  const single = targets.length === 1 ? replicasOf(targets[0].row) : undefined;
  const [count, setCount] = useState(single !== undefined ? String(single) : "");
  if (act === "suspend" || act === "resume") {
    return <SuspendDialog env={env} targets={targets} on={act === "suspend"} onClose={onClose} onDone={() => onDone?.([])} />;
  }
  const run = async () => {
    let replicas = 0;
    if (act === "scale") {
      replicas = Number(count);
      if (count.trim() === "" || !Number.isInteger(replicas) || replicas < 0) throw new Error("Replicas must be a whole number, 0 or more");
    }
    for (const t of targets) {
      const id = `${t.kind}/${t.ns}/${t.name}`;
      if (done.current.has(id)) continue;
      try {
        done.current.set(id, await runOne(env, act, t, replicas));
      } catch (e) {
        if (targets.length === 1) throw e;
        const msg = e instanceof Error ? e.message : String(e);
        throw new Error(`${done.current.size} of ${targets.length} done. ${t.name}: ${msg}`, { cause: e });
      }
    }
    // Lists follow their watches; this refreshes object pages and counts.
    await qc.invalidateQueries({ queryKey: keys.env(env) });
    onDone?.(targets.map((t) => done.current.get(`${t.kind}/${t.ns}/${t.name}`)));
  };
  const title = `${actLabel(act)} ${what(targets)}${act === "scale" ? "" : "?"}`;
  if (act === "scale") {
    return (
      <FormDialog
        title={title}
        description={targets.length > 1 ? "Each gets the same number of replicas." : undefined}
        submitLabel="Scale"
        onClose={onClose}
        onSubmit={run}
      >
        <label className="grid gap-1 text-sm">
          Replicas
          <input
            type="number"
            required
            min={0}
            step={1}
            autoFocus
            className="h-9 rounded-md bg-[#1c1c1c] px-2"
            value={count}
            onChange={(e) => setCount(e.target.value)}
          />
        </label>
      </FormDialog>
    );
  }
  return (
    <ConfirmDialog
      open
      onOpenChange={(o) => !o && onClose()}
      title={title}
      description={consequence(act, targets)}
      confirmLabel={actLabel(act)}
      // Only deleting is beyond undoing: the rest wear the cord.
      destructive={act === "delete"}
      onConfirm={run}
    />
  );
}
