import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, getRouteApi, useNavigate } from "@tanstack/react-router";
import { FileText, Info, ListTree, ScrollText, TerminalSquare } from "lucide-react";
import { lazy, Suspense, useState, type ReactNode } from "react";
import type { KubeKind, KubeRow } from "@/api/gen/types.gen";
import { useEnvAccess } from "@/api/access";
import { keys, kubeEventsQuery, kubeObjectQuery } from "@/api/queries";
import { useKubeWatch } from "@/live/kubeWatch";
import { DetailError, Details } from "@/components/Details";
import { Page } from "@/components/Page";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { formatAgo, formatCores, formatCpu, formatMemory, formatPercent } from "@/lib/format";
import { KubeActions } from "./KubeActions";
import { KubeHealth } from "./KubeHealth";
import { asKind, CLUSTER_SCOPED, KIND_COLUMNS, KIND_LABEL } from "./kinds";
import { useKubeUsage, usageKey } from "./usage";
import { LivePaused } from "./LivePaused";
import { Conditions, EventStrips } from "./ObjectParts";
import type { KubeTab } from "./tabs";

const LogsTab = lazy(() => import("@/pages/containers/logs/LogsTab").then((m) => ({ default: m.LogsTab })));
const TerminalTab = lazy(() => import("@/pages/containers/terminal/TerminalTab").then((m) => ({ default: m.TerminalTab })));
const route = getRouteApi("/authed/shell/envs/$env/k8s/$kind/$ns/$name");

export function KubeObjectPage() {
  const { env, kind: raw, ns, name } = route.useParams();
  const { tab = "summary" } = route.useSearch();
  const navigate = useNavigate();
  const kind = asKind(raw) ?? "pods";
  const { canWrite } = useEnvAccess(env);
  const obj = useQuery(kubeObjectQuery(env, kind, ns, name));
  const events = useQuery({ ...kubeEventsQuery(env, kind, ns, name), enabled: tab === "events" });
  const [container, setContainer] = useState<string | undefined>();
  const qc = useQueryClient();
  const [gone, setGone] = useState(false);
  const live = useKubeWatch(env, kind, CLUSTER_SCOPED.includes(kind) ? undefined : ns, (ev) => {
    if (ev.type === "snapshot") setGone(!ev.rows.some((r) => r.name === name));
    if ((ev.type === "added" || ev.type === "modified") && ev.row.name === name) {
      setGone(false);
      // The events key extends this one, so an open Events tab refetches too.
      void qc.invalidateQueries({ queryKey: keys.kubeObject(env, kind, ns, name) });
    }
    if (ev.type === "deleted" && ev.name === name) setGone(true);
  });
  const usage = useUsageItems(env, kind, ns, name);
  if (obj.error)
    return (
      <DetailError
        error={obj.error}
        what={KIND_LABEL[kind].replace(/s$/, "").toLowerCase()}
        back={
          <Link to="/envs/$env/k8s/$kind" params={{ env, kind }} className="text-sm underline">
            Back to {KIND_LABEL[kind]}
          </Link>
        }
      />
    );
  const o = obj.data;
  const isPod = kind === "pods";
  const chosen = container ?? o?.containers[0];
  const tabs: [KubeTab, string, typeof Info][] = [
    ["summary", "Summary", Info],
    ["yaml", "YAML", FileText],
    ["events", "Events", ListTree],
    ...(isPod ? ([["logs", "Logs", ScrollText]] as [KubeTab, string, typeof Info][]) : []),
    ...(isPod && canWrite ? ([["terminal", "Terminal", TerminalSquare]] as [KubeTab, string, typeof Info][]) : []),
  ];
  return (
    <Page title={name} meta={o && <KubeHealth health={o.row.health} label={o.row.status} />} actions={o && <KubeActions env={env} kind={kind} ns={ns} name={name} row={o.row} />}>
      {live.paused && <LivePaused paused={live.paused} onRetry={live.retry} />}
      {gone && (
        <p role="status" className="mb-3 rounded-md border border-state-fail/40 bg-state-fail/10 px-3 py-2 text-sm">
          This object was deleted.{" "}
          <Link to="/envs/$env/k8s/$kind" params={{ env, kind }} className="underline">
            Back to {KIND_LABEL[kind]}
          </Link>
        </p>
      )}
      {o && (
        <div className="mb-3">
      {o.owners.length > 0 && (
        <p className="text-sm">
          Owned by{" "}
          {o.owners.map((r, i) => (
            <span key={`${r.kind}/${r.name}`}>
              {i > 0 && " → "}
              {r.link ? (
                <Link to="/envs/$env/k8s/$kind/$ns/$name" params={{ env, kind: r.link, ns, name: r.name }} className="hover:underline">{r.name}</Link>
              ) : (
                <span>{r.name}</span>
              )}{" "}
              <span className="text-muted-foreground">({r.kind})</span>
            </span>
          ))}
        </p>
      )}
        </div>
      )}
      <Tabs value={tab} onValueChange={(t) => void navigate({ to: ".", search: { tab: t as KubeTab }, replace: true })}>
        <TabsList>
          {tabs.map(([value, label, Icon]) => (
            <TabsTrigger key={value} value={value}>
              <Icon aria-hidden strokeWidth={1.75} />
              {label}
            </TabsTrigger>
          ))}
        </TabsList>
        <TabsContent value="summary">
          {o && (
            <div className="grid gap-4">
              <Details label="Summary" items={[...summaryItems(kind, o.row), ...usage]} />
              {o.conditions.length > 0 && <Conditions conditions={o.conditions} />}
            </div>
          )}
        </TabsContent>
        <TabsContent value="yaml">
          {o && <pre aria-label="Object YAML" className="overflow-x-auto rounded-md border bg-muted/40 p-3 font-mono text-xs">{o.yaml}</pre>}
        </TabsContent>
        <TabsContent value="events">
          <EventStrips events={events.data} pending={events.isPending} error={events.error} />
        </TabsContent>
        {isPod && o && (
          <>
            <ContainerPicker containers={o.containers} value={chosen} onChange={setContainer} hidden={tab !== "logs" && tab !== "terminal"} />
            <TabsContent value="logs">
              <Suspense fallback={null}>
                <LogsTab key={chosen} env={env} id={`${ns}/${name}`} name={name} running={o.row.status === "Running"} container={chosen} />
              </Suspense>
            </TabsContent>
            {canWrite && (
              <TabsContent value="terminal">
                <Suspense fallback={null}>
                  <TerminalTab key={chosen} env={env} id={`${ns}/${name}`} running={o.row.status === "Running"} container={chosen} />
                </Suspense>
              </TabsContent>
            )}
          </>
        )}
      </Tabs>
    </Page>
  );
}

function ContainerPicker({ containers, value, onChange, hidden }: { containers: string[]; value?: string; onChange: (c: string) => void; hidden: boolean }) {
  if (hidden || containers.length < 2) return null;
  return (
    <label className="my-2 flex items-center gap-2 text-sm">
      <span className="text-muted-foreground">Container</span>
      <select aria-label="Container" className="h-8 rounded-md border bg-background px-2" value={value} onChange={(e) => onChange(e.target.value)}>
        {containers.map((c) => (<option key={c} value={c}>{c}</option>))}
      </select>
    </label>
  );
}

/** CPU and memory now as summary rows, when metrics-server reports them. */
function useUsageItems(env: string, kind: KubeKind, ns: string, name: string): [string, ReactNode][] {
  const metered = kind === "pods" || kind === "nodes";
  const usage = useKubeUsage(env, kind === "nodes" ? "nodes" : "pods", kind === "pods" ? ns : undefined, metered);
  const u = usage.index.get(usageKey(kind === "pods" ? ns : null, name));
  if (!metered || !usage.available || !u) return [];
  const of = (alloc: number | null | undefined, f: (n: number) => string, used: number) => (alloc ? ` of ${f(alloc)} (${formatPercent((used / alloc) * 100)})` : "");
  return [
    ["CPU", <span className="tabular-nums">{formatCpu(u.cpu)}{of(u.cpu_allocatable, (n) => `${formatCores(n)} cores`, u.cpu)}</span>],
    ["Memory", <span className="tabular-nums">{formatMemory(u.memory)}{of(u.memory_allocatable, formatMemory, u.memory)}</span>],
  ];
}

/** Kind, namespace and age, then the kind's own columns under the list's headers. */
function summaryItems(kind: KubeKind, row: KubeRow): [string, ReactNode][] {
  const header = new Map(KIND_COLUMNS[kind]);
  const label = (k: string) => header.get(k) ?? k.charAt(0).toUpperCase() + k.slice(1).replace(/_/g, " ");
  const value = (k: string, v: string) => (k === "last_seen" && Number(v) ? formatAgo(Number(v)) : v || "—");
  return [
    ["Kind", KIND_LABEL[kind].replace(/s$/, "")],
    ["Namespace", row.namespace ? <span className="font-ident">{row.namespace}</span> : undefined],
    ["Age", formatAgo(row.created)],
    ...Object.entries(row.columns).map(([k, v]): [string, ReactNode] => [label(k), value(k, v)]),
  ];
}
