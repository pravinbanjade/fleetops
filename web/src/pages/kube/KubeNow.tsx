import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { Trash2 } from "lucide-react";
import { useState } from "react";
import { useEnvAccess } from "@/api/access";
import { deleteGenericKube } from "@/api/gen/sdk.gen";
import type { KubeEvent, KubeKind, KubeRow, KubeUsageRow } from "@/api/gen/types.gen";
import { genericEventsQuery, kubeEventsQuery } from "@/api/queries";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { LogTail } from "@/components/LogTail";
import { ObjectNow, type NowFact, type NowRow } from "@/components/shell/ObjectNow";
import { Tick } from "@/components/Tick";
import { Button } from "@/components/ui/button";
import { formatAgo, formatCpu, formatMemory } from "@/lib/format";
import { ActDialog, actIcon, actLabel, actsFor, type Act } from "./Act";
import type { Ref } from "./generic/ref";
import { actPerm } from "./perms";
import { KIND_LABEL, POD_LOG_KINDS } from "./kinds";

/** A Kubernetes event as a panel row: its reason, and how often it happened. */
function eventRows(events: KubeEvent[] | undefined): NowRow[] {
  return (events ?? [])
    .slice()
    .sort((a, b) => (b.last_seen ?? 0) - (a.last_seen ?? 0))
    .slice(0, 30)
    .map((e, i) => ({ key: `${e.reason}-${i}`, label: `${e.reason}: ${e.message}`, aside: e.count > 1 ? `×${e.count}` : undefined, failing: e.type === "Warning" }));
}

function rowFacts(row: KubeRow & { usage?: KubeUsageRow }): NowFact[] {
  const facts: NowFact[] = [{ label: "Status", value: <span className={row.health === "bad" ? "text-fall" : undefined}>{row.status || "—"}</span> }];
  if (row.usage) {
    facts.push(
      { label: "CPU", value: <Tick value={row.usage.cpu} text={formatCpu(row.usage.cpu)} /> },
      { label: "Memory", value: <Tick value={row.usage.memory} text={formatMemory(row.usage.memory)} /> },
    );
  } else {
    facts.push({ label: "Age", value: formatAgo(row.created) });
    if (row.namespace) facts.push({ label: "Namespace", value: <span className="font-ident text-sm">{row.namespace}</span> });
  }
  return facts;
}

/**
 * A curated Kubernetes object in the panel. Pods and workloads stream their
 * logs; everything else shows its recent events.
 */
export function KubeNow({ env, kind, row }: { env: string; kind: KubeKind; row: KubeRow & { usage?: KubeUsageRow } }) {
  const ns = row.namespace ?? "-";
  const logs = kind === "pods" ? `${ns}/${row.name}` : POD_LOG_KINDS.includes(kind) ? `${kind}/${ns}/${row.name}` : null;
  const events = useQuery({ ...kubeEventsQuery(env, kind, ns, row.name), enabled: !logs });
  return (
    <ObjectNow
      title={<span className="font-ident">{row.name}</span>}
      subtitle={`${KIND_LABEL[kind]}${row.namespace ? ` · ${row.namespace}` : ""}`}
      facts={rowFacts(row)}
      list={logs ? undefined : { title: "Events", empty: "No recent events.", rows: eventRows(events.data), error: events.error, loading: events.isPending }}
      open={{ to: "/envs/$env/k8s/$kind/$ns/$name", params: { env, kind, ns, name: row.name } }}
      actions={<KubeNowActions env={env} kind={kind} row={row} />}
    >
      {logs && <LogTail env={env} id={logs} />}
    </ObjectNow>
  );
}

/**
 * The actions a list offers for this one object (Restart, Scale, Suspend,
 * Run now, Cordon, Delete), each behind its own confirmation. Deleting
 * closes the panel: the row leaves the list and the panel follows it.
 */
function KubeNowActions({ env, kind, row }: { env: string; kind: KubeKind; row: KubeRow }) {
  const navigate = useNavigate();
  const { can } = useEnvAccess(env);
  const [open, setOpen] = useState<Act | null>(null);
  const ns = row.namespace ?? "-";
  const acts = actsFor(kind, [row])
    .map((a) => a.act)
    .filter((act) => can(actPerm(kind, act), row.namespace ?? null));
  const done = (act: Act, results: (string | undefined)[]) => {
    setOpen(null);
    // Run now: the new Job is what you came to watch.
    const job = results[0];
    if (act === "trigger" && job) void navigate({ to: "/envs/$env/k8s/$kind/$ns/$name", params: { env, kind: "jobs", ns, name: job } });
  };
  return (
    <>
      {acts.map((act) => {
        const Icon = actIcon(act);
        return (
          <Button key={act} size="sm" variant={act === "delete" ? "destructive" : "secondary"} onClick={() => setOpen(act)}>
            <Icon aria-hidden />
            {actLabel(act)}
          </Button>
        );
      })}
      {open && <ActDialog env={env} act={open} targets={[{ kind, ns, name: row.name, row }]} onClose={() => setOpen(null)} onDone={(r) => done(open, r)} />}
    </>
  );
}

/** A discovered (custom) resource in the panel: its status and recent events, and Delete when allowed. */
export function GenericNow({ env, r, row, deletable }: { env: string; r: Ref; row: KubeRow; deletable: boolean }) {
  const [deleting, setDeleting] = useState(false);
  const ns = row.namespace ?? "-";
  const events = useQuery(genericEventsQuery(env, r, ns, row.name));
  return (
    <ObjectNow
      title={<span className="font-ident">{row.name}</span>}
      subtitle={`${r.resource}${row.namespace ? ` · ${row.namespace}` : ""}`}
      facts={rowFacts(row)}
      list={{ title: "Events", empty: "No recent events.", rows: eventRows(events.data), error: events.error, loading: events.isPending }}
      open={{ to: "/envs/$env/k8s/r/$group/$version/$resource/$ns/$name", params: { env, ...r, ns, name: row.name } }}
      actions={
        deletable && (
          <>
            <Button size="sm" variant="destructive" onClick={() => setDeleting(true)}>
              <Trash2 aria-hidden />
              Delete
            </Button>
            {deleting && (
              <ConfirmDialog
                open
                onOpenChange={(v) => !v && setDeleting(false)}
                title={`Delete ${row.name}?`}
                description="This cannot be undone."
                confirmLabel="Delete"
                destructive
                onConfirm={async () => {
                  await deleteGenericKube({ path: { env, ...r, ns, name: row.name }, throwOnError: true });
                  setDeleting(false);
                }}
              />
            )}
          </>
        )
      }
    />
  );
}
