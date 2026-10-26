import { useQuery } from "@tanstack/react-query";
import type { KubeEvent, KubeKind, KubeRow, KubeUsageRow } from "@/api/gen/types.gen";
import { genericEventsQuery, kubeEventsQuery } from "@/api/queries";
import { LogTail } from "@/components/LogTail";
import { ObjectNow, type NowFact, type NowRow } from "@/components/shell/ObjectNow";
import { Tick } from "@/components/Tick";
import { formatAgo, formatCpu, formatMemory } from "@/lib/format";
import type { Ref } from "./generic/ref";
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
      list={logs ? undefined : { title: "Events", empty: "No recent events.", rows: eventRows(events.data) }}
      open={{ to: "/envs/$env/k8s/$kind/$ns/$name", params: { env, kind, ns, name: row.name } }}
    >
      {logs && <LogTail env={env} id={logs} />}
    </ObjectNow>
  );
}

/** A discovered (custom) resource in the panel: its status and recent events. */
export function GenericNow({ env, r, row }: { env: string; r: Ref; row: KubeRow }) {
  const ns = row.namespace ?? "-";
  const events = useQuery(genericEventsQuery(env, r, ns, row.name));
  return (
    <ObjectNow
      title={<span className="font-ident">{row.name}</span>}
      subtitle={`${r.resource}${row.namespace ? ` · ${row.namespace}` : ""}`}
      facts={rowFacts(row)}
      list={{ title: "Events", empty: "No recent events.", rows: eventRows(events.data) }}
      open={{ to: "/envs/$env/k8s/r/$group/$version/$resource/$ns/$name", params: { env, ...r, ns, name: row.name } }}
    />
  );
}
