import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, getRouteApi, useNavigate } from "@tanstack/react-router";
import { FileText, Info, ListTree, ScrollText, TerminalSquare } from "lucide-react";
import { lazy, Suspense, useState, type ReactNode } from "react";
import type { KubeKind, KubeRef, KubeRow } from "@/api/gen/types.gen";
import { useEnvAccess } from "@/api/access";
import { kubePerm } from "./perms";
import { SecretValues } from "./SecretValues";
import { keys, kubeEventsQuery, kubeObjectQuery } from "@/api/queries";
import { useKubeWatch } from "@/live/kubeWatch";
import { DetailError, Details } from "@/components/Details";
import { Page } from "@/components/Page";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { formatAgo, formatCores, formatCpu, formatMemory, formatPercent } from "@/lib/format";
import { KubeActions } from "./KubeActions";
import { WorkloadTerminal } from "./WorkloadTerminal";
import { KubeHealth } from "./KubeHealth";
import { asKind, CLUSTER_SCOPED, KIND_COLUMNS, KIND_LABEL, LINK_COLUMNS, POD_LOG_KINDS, READ_ONLY } from "./kinds";
import { useKubeUsage, usageKey } from "./usage";
import { LivePaused } from "./LivePaused";
import { Conditions, EventStrips } from "./ObjectParts";
import { ContainerChips, ContainerSquares, parseContainers } from "./ContainerSquares";
import type { KubeTab } from "./tabs";

// Reads the object's YAML (and the YAML library) only once the summary shows.
const ObjectSpec = lazy(() => import("./spec/ObjectSpec"));
const LogsTab = lazy(() => import("@/pages/containers/logs/LogsTab").then((m) => ({ default: m.LogsTab })));
const TerminalTab = lazy(() => import("@/pages/containers/terminal/TerminalTab").then((m) => ({ default: m.TerminalTab })));
const route = getRouteApi("/authed/shell/envs/$env/k8s/$kind/$ns/$name");

export function KubeObjectPage() {
  const { env, kind: raw, ns, name } = route.useParams();
  const { tab = "summary", container } = route.useSearch();
  const navigate = useNavigate();
  const kind = asKind(raw) ?? "pods";
  const { can } = useEnvAccess(env);
  const where = ns === "-" ? null : ns;
  const canShell = can("pods.shell", where);
  const obj = useQuery(kubeObjectQuery(env, kind, ns, name));
  const events = useQuery({ ...kubeEventsQuery(env, kind, ns, name), enabled: tab === "events" });
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
  const isWorkload = POD_LOG_KINDS.includes(kind);
  const setContainer = (c: string) => void navigate({ to: ".", search: (s) => ({ ...s, container: c }), replace: true });
  // Logs read any container, init ones included; a shell needs one of the pod's own.
  const listed = parseContainers(o?.row.columns.containers);
  const all = listed.length ? listed : (o?.containers ?? []).map((c) => ({ name: c, init: false, state: "unknown" as const, reason: "", message: "", restarts: 0 }));
  const pickable = tab === "terminal" ? all.filter((c) => !c.init) : all;
  const chosen = pickable.find((c) => c.name === container)?.name ?? pickable[0]?.name ?? o?.containers[0];
  // Whether the chosen container runs, not the pod: a crashing sidecar marks the pod, not its neighbours.
  const state = listed.find((c) => c.name === chosen && (tab !== "terminal" || !c.init))?.state;
  const running = state ? state === "ready" || state === "running" : o?.row.status === "Running";
  const tabs: [KubeTab, string, typeof Info][] = [
    ["summary", "Summary", Info],
    ["yaml", "YAML", FileText],
    ["events", "Events", ListTree],
    ...(isPod || isWorkload ? ([["logs", "Logs", ScrollText]] as [KubeTab, string, typeof Info][]) : []),
    ...(isPod && canShell ? ([["terminal", "Terminal", TerminalSquare]] as [KubeTab, string, typeof Info][]) : []),
  ];
  return (
    <Page kicker={KIND_LABEL[kind]} title={name} meta={o && <Meta env={env} ns={ns} health={<KubeHealth health={o.row.health} label={o.row.status} />} owners={o.owners} />} actions={
        o && (
          <div className="flex items-center gap-2">
            {isWorkload && canShell && <WorkloadTerminal env={env} kind={kind} ns={ns} name={name} />}
            <KubeActions env={env} kind={kind} ns={ns} name={name} row={o.row} />
          </div>
        )
      }>
      {live.paused && <LivePaused paused={live.paused} onRetry={live.retry} />}
      {gone && (
        <p role="status" className="mb-3 rounded-md bg-state-fail/10 px-3 py-2 text-sm">
          This object was deleted.{" "}
          <Link to="/envs/$env/k8s/$kind" params={{ env, kind }} className="underline">
            Back to {KIND_LABEL[kind]}
          </Link>
        </p>
      )}
      <Tabs value={tab} onValueChange={(t) => void navigate({ to: ".", search: (s) => ({ ...s, tab: t as KubeTab }), replace: true })}>
        <TabsList>
          {tabs.map(([value, label, Icon]) => (
            <TabsTrigger key={value} value={value}>
              <Icon aria-hidden strokeWidth={1.75} />
              {label}
            </TabsTrigger>
          ))}
        </TabsList>
        {/* Kept mounted while another tab shows, so an env or data edit in progress survives a look at the YAML. */}
        <TabsContent value="summary" forceMount className="data-[state=inactive]:hidden">
          {o && (
            <div className="grid gap-4">
              <Details label="Summary" items={[...summaryItems(env, kind, o.row), ...usage]} />
              {kind === "secrets" && can("secrets.reveal", where) && <SecretValues env={env} ns={ns} name={name} />}
              <Suspense fallback={o.conditions.length > 0 && <Conditions conditions={o.conditions} />}>
                <ObjectSpec env={env} kind={kind} ns={ns} name={name} yaml={o.yaml} canWrite={can(kubePerm(kind, "operate"), where) && !READ_ONLY.includes(kind)} owners={o.owners}>
                  {o.conditions.length > 0 && <Conditions conditions={o.conditions} />}
                </ObjectSpec>
              </Suspense>
            </div>
          )}
        </TabsContent>
        <TabsContent value="yaml">
          {o && <pre aria-label="Object YAML" className="overflow-x-auto rounded-md bg-[#0a0a0a] p-3 font-mono text-xs">{o.yaml}</pre>}
        </TabsContent>
        <TabsContent value="events">
          <EventStrips events={events.data} pending={events.isPending} error={events.error} />
        </TabsContent>
        {isPod && o && (
          <>
            {(tab === "logs" || tab === "terminal") && pickable.length > 1 && <ContainerChips containers={pickable} value={chosen} onChange={setContainer} />}
            <TabsContent value="logs">
              <Suspense fallback={null}>
                <LogsTab
                  key={chosen}
                  env={env}
                  id={`${ns}/${name}`}
                  name={name}
                  running={running}
                  container={chosen}
                  canPrevious={(listed.find((c) => c.name === chosen)?.restarts ?? 0) > 0}
                />
              </Suspense>
            </TabsContent>
            {canShell && (
              <TabsContent value="terminal">
                <Suspense fallback={null}>
                  <TerminalTab key={chosen} env={env} id={`${ns}/${name}`} running={running} container={chosen} />
                </Suspense>
              </TabsContent>
            )}
          </>
        )}
        {isWorkload && o && (
          <>
            {tab === "logs" && pickable.length > 1 && <ContainerChips containers={pickable} value={chosen} onChange={setContainer} />}
            <TabsContent value="logs">
              <Suspense fallback={null}>
                <LogsTab key={chosen} env={env} id={`${kind}/${ns}/${name}`} name={name} running container={chosen} podPrefix={`${name}-`} canPrevious />
              </Suspense>
            </TabsContent>
          </>
        )}
      </Tabs>
    </Page>
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

/** The state, then who owns it: "Owned by ReplicaSet api-7d9 → Deployment api". */
function Meta({ env, ns, health, owners }: { env: string; ns: string; health: ReactNode; owners: KubeRef[] }) {
  return (
    <span className="flex flex-wrap items-center gap-x-3 gap-y-1">
      {health}
      {owners.length > 0 && (
        <span className="min-w-0">
          Owned by{" "}
          {owners.map((r, i) => (
            <span key={`${r.kind}/${r.name}`}>
              {i > 0 && <span aria-hidden> → </span>}
              {i > 0 && <span className="sr-only">, owned by </span>}
              {r.kind}{" "}
              {r.link ? (
                <Link to="/envs/$env/k8s/$kind/$ns/$name" params={{ env, kind: r.link, ns, name: r.name }} className="font-ident text-foreground underline decoration-border underline-offset-4 hover:decoration-primary">
                  {r.name}
                </Link>
              ) : (
                <span className="font-ident text-foreground">{r.name}</span>
              )}
            </span>
          ))}
        </span>
      )}
    </span>
  );
}

/** Columns that only tell a list where a cell links; the cell itself shows the name. */
const HIDDEN_COLUMNS = new Set(["claim_namespace", "target_kind", "role_kind", "object_link", "object_namespace", "unschedulable"]);

/** Columns whose values the cluster wrote (addresses, node and object names), set in mono. */
const IDENT_COLUMNS = new Set(["node", "ip", "internal_ip", "cluster_ip", "external_ip", "address", "hosts", "volume", "claim", "owner", "storage_class", "provisioner", "pod_selector", "target", "role", "version"]);

/** Kind, namespace and age, then the kind's own columns in the list's order and under its headers. */
function summaryItems(env: string, kind: KubeKind, row: KubeRow): [string, ReactNode][] {
  const header = new Map(KIND_COLUMNS[kind]);
  const label = (k: string) => header.get(k) ?? k.charAt(0).toUpperCase() + k.slice(1).replace(/_/g, " ");
  const keys = [...KIND_COLUMNS[kind].map(([k]) => k).filter((k) => k in row.columns), ...Object.keys(row.columns).filter((k) => !header.has(k) && !HIDDEN_COLUMNS.has(k))];
  const value = (k: string, v: string): ReactNode => {
    if (!v) return "—";
    if (k === "last_seen" && Number(v)) return formatAgo(Number(v));
    if (k === "containers") return <ContainerSquares column={v} ready={row.columns.ready} />;
    const to = LINK_COLUMNS[kind]?.[k]?.(row.columns, row.namespace ?? null);
    if (to)
      return (
        <Link to="/envs/$env/k8s/$kind/$ns/$name" params={{ env, kind: to.kind, ns: to.ns, name: v }} className="font-ident underline decoration-border underline-offset-4 hover:decoration-primary">
          {v}
        </Link>
      );
    return IDENT_COLUMNS.has(k) ? <span className="font-ident font-normal">{v}</span> : v;
  };
  return [
    ["Kind", KIND_LABEL[kind].replace(/s$/, "")],
    [
      "Namespace",
      row.namespace ? (
        <Link to="/envs/$env/k8s/$kind/$ns/$name" params={{ env, kind: "namespaces", ns: "-", name: row.namespace }} className="font-ident underline decoration-border underline-offset-4 hover:decoration-primary">
          {row.namespace}
        </Link>
      ) : undefined,
    ],
    ["Age", <time dateTime={new Date(row.created * 1000).toISOString()} title={new Date(row.created * 1000).toLocaleString()}>{formatAgo(row.created)}</time>],
    ...keys.map((k): [string, ReactNode] => [label(k), value(k, row.columns[k])]),
  ];
}
