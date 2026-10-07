import { useQuery, type UseQueryResult } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import {
  Boxes,
  CircleCheck,
  Cpu,
  FileCog,
  Flame,
  FolderTree,
  Globe,
  History,
  KeyRound,
  MemoryStick,
  Pause,
  Package,
  TriangleAlert,
  Waypoints,
  type LucideIcon,
} from "lucide-react";
import { useId, useState, type ReactNode } from "react";
import type { HelmRelease, KubeCount, KubeFilter, KubeKind, KubeList, KubeRow, KubeUsageRow, RowHealth } from "@/api/gen/types.gen";
import { ApiError } from "@/api/client";
import { envStatusQuery, helmReleasesQuery, kubeCountsQuery, kubeListQuery } from "@/api/queries";
import { Page } from "@/components/Page";
import { Readout } from "@/components/Readout";
import { StateBadge, type Tone } from "@/components/StateBadge";
import { walk } from "@/components/walk";
import { errorMessage } from "@/lib/errors";
import { formatAgo, formatCores, formatCpu, formatMemory, formatPercent } from "@/lib/format";
import { cn } from "@/lib/utils";
import { useLive } from "@/live/LiveProvider";
import { useKubeWatch, type WatchPause } from "@/live/kubeWatch";
import { HEALTH_RANK, KIND_LABEL, SUSPENDABLE, asKind } from "./kinds";
import { SuspendDialog, type SuspendTarget } from "./Suspend";
import { useEnvAccess } from "@/api/access";
import { Meter } from "./Meter";
import { LivePaused } from "./LivePaused";
import { NamespacePicker, useNamespace } from "./NamespacePicker";
import { clusterTotals, share, useKubeUsage, useSeries } from "./usage";

/** A row's health as a holder tone: the same readings the Docker board uses. */
const TONE: Record<RowHealth, Tone> = { bad: "fail", warn: "wait", ok: "run", unknown: "idle", off: "idle" };
const FILL: Record<RowHealth, string> = { bad: "bg-state-fail", warn: "bg-state-wait", ok: "bg-state-run", unknown: "bg-input", off: "bg-muted-foreground/35" };

/** Strips shown in the attention bay before the full lists take over. */
const ATTENTION_SHOWN = 10;
const NODES_SHOWN = 8;
const WARNINGS_SHOWN = 8;
const BUSIEST_SHOWN = 6;
/** Usage readings a readout's line holds: five minutes at metrics-server's 15s. */
const SERIES = 20;

type Live = { q: UseQueryResult<KubeList>; paused: WatchPause | null; retry: () => void; forbidden: boolean };

/** A kind's list, kept live from the cluster's watch once its snapshot lands. */
function useLiveKind(env: string, kind: KubeKind, ns: string | undefined, filter?: KubeFilter): Live {
  const live = useKubeWatch(env, kind, ns, undefined, filter);
  const q = useQuery({ ...kubeListQuery(env, kind, ns, filter), enabled: !live.synced });
  const forbidden = q.error instanceof ApiError && q.error.status === 403;
  return { q, paused: forbidden ? null : live.paused, retry: live.retry, forbidden };
}

/**
 * A cluster's home as a strip board: what needs someone first (failing pods,
 * workloads short of replicas, failed releases, nodes not ready), then how
 * each kind of workload is doing, the nodes, and the rest of the inventory.
 * Pods, workloads and nodes follow the cluster's watches.
 */
export function KubeOverviewPage({ env }: { env: string }) {
  const [ns, setNs] = useNamespace(env);
  const status = useQuery(envStatusQuery(env, true));
  const pods = useLiveKind(env, "pods", ns);
  const deployments = useLiveKind(env, "deployments", ns);
  const statefulsets = useLiveKind(env, "statefulsets", ns);
  const daemonsets = useLiveKind(env, "daemonsets", ns);
  const jobs = useLiveKind(env, "jobs", ns);
  const cronjobs = useLiveKind(env, "cronjobs", ns);
  const nodes = useLiveKind(env, "nodes", undefined);
  // Only warnings, filtered by the cluster: a busy cluster writes many
  // Normal events a second, and this panel shows none of them.
  const events = useLiveKind(env, "events", ns, "warnings");
  const helm = useQuery(helmReleasesQuery(env, ns));
  const nodeUsage = useKubeUsage(env, "nodes");
  const podUsage = useKubeUsage(env, "pods", ns);
  const workloads: [KubeKind, Live][] = [
    ["deployments", deployments],
    ["statefulsets", statefulsets],
    ["daemonsets", daemonsets],
    ["jobs", jobs],
    ["cronjobs", cronjobs],
  ];
  const lives = [pods, nodes, events, ...workloads.map(([, l]) => l)];
  const paused = lives.find((l) => l.paused)?.paused ?? null;
  const s = status.data;
  const down = s?.state === "down" ? s.reason : null;
  const version = s?.state === "up" && s.version.kind === "kube" ? s.version : undefined;
  const where = ns ? `namespace ${ns}` : "all namespaces";

  return (
    <Page
      title={env}
      meta={
        <span className="flex flex-wrap items-center gap-x-4 gap-y-1">
          <Liveness down={down !== null} paused={paused !== null} />
          {version && (
            <span className="flex flex-wrap items-center gap-x-2">
              <span>
                Kubernetes <span className="font-ident text-foreground">{version.git_version}</span>
              </span>
              <span aria-hidden>·</span>
              <span className="font-ident">{version.platform}</span>
              <span aria-hidden>·</span>
              <span>{version.nodes == null ? "Nodes not visible to these credentials" : `${version.nodes_ready} of ${version.nodes} nodes ready`}</span>
            </span>
          )}
          {nodeUsage.reason && <span>No CPU or memory figures: {nodeUsage.reason}.</span>}
        </span>
      }
      actions={
        <div className="flex w-full flex-wrap items-end justify-end gap-2 sm:w-auto xl:gap-x-5">
          <NamespacePicker env={env} value={ns} onChange={setNs} />
          {nodeUsage.available && <ClusterUsage usage={nodeUsage} />}
        </div>
      }
    >
      {down !== null && (
        <div role="alert" className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1.5 rounded-lg border border-destructive/50 bg-destructive/8 p-4">
          <TriangleAlert aria-hidden className="row-span-2 mt-0.5 size-5 text-destructive" />
          <p className="font-semibold text-destructive">This cluster is unavailable</p>
          <p className="font-mono text-xs break-all">{down}</p>
        </div>
      )}
      {paused && <LivePaused paused={paused} onRetry={() => lives.forEach((l) => l.paused && l.retry())} />}
      <div className="grid gap-5 xl:grid-cols-[minmax(0,1fr)_24rem]">
        <div className="grid min-w-0 content-start gap-5">
          <AttentionPanel env={env} pods={pods} workloads={workloads} nodes={nodes} helm={helm.data} where={where} />
          <WarningsPanel env={env} events={events} where={where} />
          <WorkloadsPanel env={env} workloads={workloads} />
        </div>
        <div className="grid min-w-0 content-start gap-5">
          <NodesPanel env={env} nodes={nodes} usage={nodeUsage.available ? nodeUsage.index : undefined} />
          {podUsage.available && <BusiestPanel env={env} rows={podUsage.rows} />}
          <InventoryPanel env={env} ns={ns} helm={helm} />
        </div>
      </div>
    </Page>
  );
}

/** Whether what's on screen follows the cluster, in words. */
function Liveness({ down, paused }: { down: boolean; paused: boolean }) {
  const { state } = useLive();
  const [tone, text] = down
    ? ["bg-state-fail", "Unavailable. Showing the last known state."]
    : paused
      ? ["bg-state-wait", "Live updates paused for part of this page."]
      : state === "reconnecting"
        ? ["bg-state-wait", "Reconnecting. Showing data loaded earlier."]
        : state === "connecting"
          ? ["bg-muted-foreground", "Connecting…"]
          : ["bg-live", "Live. Updates as the cluster reports changes."];
  return (
    <p data-testid="liveness" aria-live="polite" className="flex items-center gap-2">
      <span aria-hidden className={cn("size-2 shrink-0 rounded-full", tone)} />
      {text}
    </p>
  );
}

/** One thing that needs someone, whatever kind it is. */
type Trouble = {
  key: string;
  what: string;
  name: string;
  namespace?: string | null;
  status: string;
  tone: Tone;
  rank: number;
  detail?: string;
  link: ReactNode;
  /** A failing workload that can be parked from here. */
  suspend?: SuspendTarget;
};

function troubleLink(env: string, kind: KubeKind, r: KubeRow, label: string) {
  return (
    <Link
      data-strip
      to="/envs/$env/k8s/$kind/$ns/$name"
      params={{ env, kind, ns: r.namespace ?? "-", name: r.name }}
      search={kind === "pods" ? { tab: "logs" } : {}}
      className="truncate font-ident font-medium hover:underline focus-visible:outline-none"
    >
      {label}
    </Link>
  );
}

function AttentionPanel({
  env,
  pods,
  workloads,
  nodes,
  helm,
  where,
}: {
  env: string;
  pods: Live;
  workloads: [KubeKind, Live][];
  nodes: Live;
  helm: HelmRelease[] | undefined;
  where: string;
}) {
  const id = useId();
  const { canWrite } = useEnvAccess(env);
  const [parking, setParking] = useState<SuspendTarget | null>(null);
  const podRows = pods.q.data?.rows;
  if (pods.q.isPending && !pods.forbidden) return <Loading label="Loading pods" />;
  const trouble: Trouble[] = [];
  const fromRows = (kind: KubeKind, rows: KubeRow[] | undefined, what: string, detail?: (r: KubeRow) => string | undefined) => {
    for (const r of rows ?? []) {
      if (r.health !== "bad" && r.health !== "warn") continue;
      trouble.push({
        key: `${kind}/${r.namespace ?? "-"}/${r.name}`,
        what,
        name: r.name,
        namespace: r.namespace,
        status: r.status,
        tone: TONE[r.health],
        rank: HEALTH_RANK[r.health],
        detail: detail?.(r),
        link: troubleLink(env, kind, r, r.name),
        suspend: SUSPENDABLE.includes(kind) && r.namespace ? { kind, ns: r.namespace, name: r.name, row: r } : undefined,
      });
    }
  };
  fromRows("nodes", nodes.q.data?.rows, "Node");
  fromRows("pods", podRows, "Pod", (r) => {
    const n = Number(r.columns.restarts ?? 0);
    return n > 0 ? `${n} ${n === 1 ? "restart" : "restarts"}` : undefined;
  });
  for (const [kind, l] of workloads) fromRows(kind, l.q.data?.rows, KIND_LABEL[kind].replace(/s$/, ""), (r) => r.columns.ready && `${r.columns.ready} ready`);
  for (const h of helm ?? []) {
    const bad = h.status === "failed" || h.status === "unreadable";
    if (!bad && !h.status.startsWith("pending-")) continue;
    trouble.push({
      key: `helm/${h.namespace}/${h.name}`,
      what: "Helm release",
      name: h.name,
      namespace: h.namespace,
      status: h.status,
      tone: bad ? "fail" : "wait",
      rank: bad ? 0 : 1,
      detail: `revision ${h.revision}`,
      link: (
        <Link
          data-strip
          to="/envs/$env/k8s/helm/$ns/$name"
          params={{ env, ns: h.namespace, name: h.name }}
          className="truncate font-ident font-medium hover:underline focus-visible:outline-none"
        >
          {h.name}
        </Link>
      ),
    });
  }
  trouble.sort((a, b) => a.rank - b.rank || a.name.localeCompare(b.name));
  const shown = trouble.slice(0, ATTENTION_SHOWN);
  const total = podRows?.length ?? 0;
  const headline =
    trouble.length === 0
      ? total === 0 && !pods.forbidden
        ? `No pods in ${where}`
        : "Nothing needs attention"
      : `${trouble.length} ${trouble.length === 1 ? "thing needs" : "things need"} attention`;
  const Icon = trouble.length ? TriangleAlert : CircleCheck;
  // Pod phases, most urgent first, as the board's footer.
  const phases = new Map<string, { n: number; health: RowHealth }>();
  for (const p of podRows ?? []) {
    const e = phases.get(p.status);
    if (e) e.n += 1;
    else phases.set(p.status, { n: 1, health: p.health });
  }
  const counts = [...phases].sort(([, a], [, b]) => HEALTH_RANK[a.health] - HEALTH_RANK[b.health] || b.n - a.n);
  return (
    <section aria-labelledby={id} className="overflow-hidden rounded-lg border bg-card">
      <div className="flex flex-wrap items-center gap-x-2.5 gap-y-1 border-b px-4 py-2.5">
        <Icon aria-hidden className={cn("size-4.5", trouble.length ? "text-state-fail" : "text-state-run")} />
        <h2 id={id} className="font-callsign text-base font-semibold">
          {headline}
        </h2>
        {shown.length > 1 && (
          <span className="ml-auto hidden text-xs text-muted-foreground sm:inline">
            <kbd className="font-mono">j</kbd> <kbd className="font-mono">k</kbd> to move, <kbd className="font-mono">↵</kbd> opens
          </span>
        )}
      </div>
      {pods.forbidden && (
        <p className="border-b px-4 py-2.5 text-sm text-muted-foreground">No permission to list pods in {where}. Pick a namespace these credentials can read.</p>
      )}
      {shown.length > 0 && (
        <ul aria-label="Needs attention" onKeyDown={walk} className="font-strip @container border-b text-sm">
          {shown.map((t) => (
            <li
              key={t.key}
              className="grid min-w-0 grid-cols-[auto_minmax(0,1fr)] items-center gap-x-3 gap-y-0.5 border-b px-4 py-2.5 last:border-b-0 strip-focus @2xl:grid-cols-[auto_minmax(0,1fr)_7rem_minmax(0,10rem)_8rem_4.5rem]"
            >
              <StateBadge state={t.status} tone={t.tone} track={t.key} />
              {t.link}
              {/* One meta line on a narrow bay; its own columns on a wide one. */}
              <span className="col-start-2 flex min-w-0 items-baseline gap-x-2 text-xs text-muted-foreground @2xl:contents @2xl:text-sm">
                <span className="shrink-0 truncate">{t.what}</span>
                <span className="truncate font-ident text-xs">{t.namespace ?? ""}</span>
                <span className="ml-auto shrink-0 tabular-nums @2xl:ml-0 @2xl:text-right">{t.detail ?? ""}</span>
                {/* Above the strip's stretched link, so it takes its own click. */}
                <span className="relative z-10 shrink-0 @2xl:justify-self-end">
                  {canWrite && t.suspend && (
                    <button
                      type="button"
                      onClick={() => setParking(t.suspend!)}
                      aria-label={`Suspend ${t.what.toLowerCase()} ${t.name}`}
                      className="-my-1 inline-flex h-7 items-center gap-1 rounded-sm px-1.5 text-xs text-muted-foreground transition-colors outline-none hover:bg-accent hover:text-foreground focus-visible:ring-[3px] focus-visible:ring-ring/35"
                    >
                      <Pause aria-hidden className="size-3.5" strokeWidth={2} />
                      {/* Icon only on a narrow bay: the meta line keeps its namespace. */}
                      <span className="@max-2xl:sr-only">Suspend</span>
                    </button>
                  )}
                </span>
              </span>
            </li>
          ))}
        </ul>
      )}
      {parking && <SuspendDialog env={env} targets={[parking]} on onClose={() => setParking(null)} onDone={() => setParking(null)} />}
      {trouble.length > shown.length && (
        <p className="border-b px-4 py-2 text-xs text-muted-foreground">
          {trouble.length - shown.length} more not shown.{" "}
          <Link to="/envs/$env/k8s/$kind" params={{ env, kind: "pods" }} className="text-foreground underline underline-offset-4">
            Open pods
          </Link>
        </p>
      )}
      {total > 0 && (
        <div className="grid gap-2.5 bg-muted/25 px-4 py-3">
          <div aria-hidden className="flex h-1.5 gap-px overflow-hidden rounded-full bg-muted">
            {counts.map(([phase, { n, health }]) => (
              <span key={phase} className={FILL[health]} style={{ width: `${((n / total) * 100).toFixed(2)}%` }} />
            ))}
          </div>
          <ul aria-label="Pods by status" className="-mx-1.5 flex flex-wrap gap-1">
            {counts.map(([phase, { n, health }]) => (
              <li key={phase}>
                <Link
                  to="/envs/$env/k8s/$kind"
                  params={{ env, kind: "pods" }}
                  search={{ q: phase }}
                  className="inline-flex h-8 items-center gap-2 rounded-md px-1.5 text-sm transition-colors hover:bg-accent"
                >
                  <span className="font-callsign text-base font-semibold tabular-nums">{n}</span>
                  <StateBadge state={phase} tone={TONE[health]} />
                </Link>
              </li>
            ))}
          </ul>
        </div>
      )}
    </section>
  );
}

/** Each kind of workload as one strip: how many, and how many are short. */
function WorkloadsPanel({ env, workloads }: { env: string; workloads: [KubeKind, Live][] }) {
  return (
    <Section title="Workloads" icon={Boxes} flush>
      <ul aria-label="Workloads by kind" className="font-strip @container text-sm">
        {workloads.map(([kind, l]) => {
          const rows = l.q.data?.rows ?? [];
          const by = { ok: 0, warn: 0, bad: 0, unknown: 0, off: 0 } satisfies Record<RowHealth, number>;
          for (const r of rows) by[r.health] += 1;
          return (
            <li
              key={kind}
              className="grid grid-cols-[minmax(0,1fr)_auto] items-center gap-x-4 gap-y-1.5 border-b px-4 py-2.5 last:border-b-0 @lg:grid-cols-[9rem_3rem_minmax(0,1fr)_12rem]"
            >
              <Link to="/envs/$env/k8s/$kind" params={{ env, kind }} className="font-medium hover:underline">
                {KIND_LABEL[kind]}
              </Link>
              <span className="font-callsign text-right text-base font-semibold tabular-nums @lg:text-left">
                {l.forbidden ? "—" : l.q.isPending ? "" : rows.length}
              </span>
              <span aria-hidden className="col-span-2 flex h-1.5 gap-px overflow-hidden rounded-full bg-muted @lg:col-span-1">
                {rows.length > 0 &&
                  (["bad", "warn", "ok", "unknown", "off"] as const).map((h) =>
                    by[h] ? <span key={h} className={FILL[h]} style={{ width: `${((by[h] / rows.length) * 100).toFixed(2)}%` }} /> : null,
                  )}
              </span>
              <span className="col-span-2 text-xs text-muted-foreground @lg:col-span-1 @lg:text-right">
                {l.forbidden
                  ? "No permission"
                  : l.q.error
                    ? errorMessage(l.q.error)
                    : rows.length === 0
                      ? l.q.isPending
                        ? "Loading…"
                        : "None"
                      : [by.bad && `${by.bad} failing`, by.warn && `${by.warn} changing`, by.ok && `${by.ok} healthy`, by.off && `${by.off} suspended`].filter(Boolean).join(" · ")}
              </span>
            </li>
          );
        })}
      </ul>
    </Section>
  );
}

function NodesPanel({ env, nodes, usage }: { env: string; nodes: Live; usage?: Map<string, KubeUsageRow> }) {
  const rows = [...(nodes.q.data?.rows ?? [])].sort((a, b) => HEALTH_RANK[a.health] - HEALTH_RANK[b.health] || a.name.localeCompare(b.name));
  const ready = rows.filter((r) => r.health === "ok").length;
  return (
    <Section
      title="Nodes"
      icon={Cpu}
      aside={
        rows.length > NODES_SHOWN ? (
          <Link to="/envs/$env/k8s/$kind" params={{ env, kind: "nodes" }} className="-mx-1 -my-1.5 inline-flex min-h-8 items-center rounded-sm px-1 hover:text-foreground hover:underline">
            All {rows.length}
          </Link>
        ) : rows.length ? (
          `${ready} of ${rows.length} ready`
        ) : undefined
      }
      flush
    >
      {nodes.forbidden ? (
        <p className="px-4 py-3 text-sm text-muted-foreground">Not visible to these credentials.</p>
      ) : nodes.q.isPending ? (
        <div className="p-4">
          <Loading label="Loading nodes" bare />
        </div>
      ) : rows.length === 0 ? (
        <p className="px-4 py-3 text-sm text-muted-foreground">{nodes.q.error ? errorMessage(nodes.q.error) : "No nodes reported."}</p>
      ) : (
        <>
          {usage && (
            <div aria-hidden className={cn(NODE_GRID, "border-b bg-muted/25 py-1.5 text-xs text-muted-foreground")}>
              <span className="col-span-2">Node</span>
              <span>CPU</span>
              <span>Memory</span>
            </div>
          )}
          <ul aria-label="Nodes" onKeyDown={walk} className="font-strip text-sm">
            {rows.slice(0, NODES_SHOWN).map((r) => {
              const u = usage?.get(`-/${r.name}`);
              return (
                <li key={r.name} className={cn(usage ? NODE_GRID : "grid grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-x-3 gap-y-0.5 px-4", "border-b py-2.5 last:border-b-0 strip-focus")}>
                  <StateBadge state={r.status} tone={TONE[r.health]} track={`nodes/${r.name}`} />
                  <Link
                    data-strip
                    to="/envs/$env/k8s/$kind/$ns/$name"
                    params={{ env, kind: "nodes", ns: "-", name: r.name }}
                    className="truncate font-ident font-medium hover:underline focus-visible:outline-none"
                  >
                    {r.name}
                  </Link>
                  {usage ? (
                    <>
                      <NodeMeter label="CPU" used={u?.cpu} alloc={u?.cpu_allocatable} title={u && `${formatCpu(u.cpu)}${u.cpu_allocatable ? ` of ${formatCores(u.cpu_allocatable)} cores` : ""}`} />
                      <NodeMeter label="Memory" used={u?.memory} alloc={u?.memory_allocatable} title={u && `${formatMemory(u.memory)}${u.memory_allocatable ? ` of ${formatMemory(u.memory_allocatable)}` : ""}`} />
                    </>
                  ) : (
                    <span className="font-ident text-xs text-muted-foreground">{r.columns.version ?? ""}</span>
                  )}
                  <span className={cn("col-start-2 truncate text-xs text-muted-foreground", usage ? "col-end-5" : "col-end-4")}>
                    {[r.columns.roles, r.columns.internal_ip, usage && r.columns.version].filter((v) => v && v !== "<none>").join(" · ")}
                  </span>
                </li>
              );
            })}
          </ul>
        </>
      )}
    </Section>
  );
}

const NODE_GRID = "grid grid-cols-[auto_minmax(0,1fr)_3.5rem_3.5rem] items-center gap-x-3 gap-y-0.5 px-4";

function NodeMeter({ label, used, alloc, title }: { label: string; used?: number; alloc?: number | null; title?: string }) {
  if (used === undefined) return <span className="text-xs text-muted-foreground">—</span>;
  const s = share(used, alloc);
  return (
    <Meter
      className="text-xs"
      value={s === undefined ? (label === "CPU" ? formatCpu(used) : formatMemory(used)) : formatPercent(s * 100)}
      share={s}
      title={title ? `${label}: ${title}` : undefined}
    />
  );
}

/** The whole cluster's CPU and memory against what its nodes can allocate. */
function ClusterUsage({ usage }: { usage: ReturnType<typeof useKubeUsage> }) {
  const t = clusterTotals(usage.rows);
  const at = usage.q.dataUpdatedAt;
  const cpu = useSeries(t.cpuShare === undefined ? undefined : t.cpuShare * 100, at, SERIES);
  const mem = useSeries(t.memory, at, SERIES);
  return (
    <section aria-label="Cluster usage" className="grid w-full min-w-0 grid-cols-2 gap-2 sm:w-auto xl:w-[24rem]">
      <Readout
        label="CPU"
        icon={Cpu}
        value={t.cpuShare === undefined ? formatCpu(t.cpu) : formatPercent(t.cpuShare * 100)}
        detail={t.cpuAlloc ? `of ${formatCores(t.cpuAlloc)} cores` : "in use"}
        share={t.cpuShare}
        values={cpu}
        max={Math.max(5, ...cpu) * 1.25}
        span="the last few minutes"
        window={SERIES}
      />
      <Readout
        label="Memory"
        icon={MemoryStick}
        value={formatMemory(t.memory)}
        detail={t.memAlloc ? `of ${formatMemory(t.memAlloc)}` : "in use"}
        share={t.memShare}
        values={mem}
        max={t.memAlloc || Math.max(1, ...mem) * 1.25}
        span="the last few minutes"
        window={SERIES}
      />
      <p className="sr-only">
        Used by all {usage.rows.length} {usage.rows.length === 1 ? "node" : "nodes"}, from metrics-server.
      </p>
    </section>
  );
}

/** The newest warnings the cluster has written, as they arrive. */
function WarningsPanel({ env, events, where }: { env: string; events: Live; where: string }) {
  const warnings = (events.q.data?.rows ?? [])
    .filter((e) => e.status === "Warning")
    .sort((a, b) => Number(b.columns.last_seen ?? 0) - Number(a.columns.last_seen ?? 0));
  return (
    <Section
      title="Recent warnings"
      icon={History}
      aside={
        <Link to="/envs/$env/k8s/$kind" params={{ env, kind: "events" }} className="-mx-1 -my-1.5 inline-flex min-h-8 items-center rounded-sm px-1 hover:text-foreground hover:underline">
          {warnings.length > WARNINGS_SHOWN ? `All ${warnings.length} warnings` : "All events"}
        </Link>
      }
      flush
    >
      {events.forbidden ? (
        <p className="px-4 py-3 text-sm text-muted-foreground">No permission to list events in {where}.</p>
      ) : events.q.isPending ? (
        <div className="p-4">
          <Loading label="Loading events" bare />
        </div>
      ) : warnings.length === 0 ? (
        <p className="px-4 py-3 text-sm text-muted-foreground">{events.q.error ? errorMessage(events.q.error) : `No warnings in ${where}.`}</p>
      ) : (
        <ul aria-label="Recent warnings" onKeyDown={walk} className="font-strip text-sm">
          {warnings.slice(0, WARNINGS_SHOWN).map((e) => {
            const c = e.columns;
            const kind = asKind(c.object_link ?? "");
            const count = Number(c.count ?? 1);
            const object = (
              <>
                <span className="text-muted-foreground">{c.object_kind}/</span>
                {c.object_name}
              </>
            );
            return (
              <li
                key={`${e.namespace ?? "-"}/${e.name}`}
                className="grid grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-x-3 gap-y-0.5 border-b px-4 py-2.5 last:border-b-0 strip-focus"
              >
                <StateBadge state={c.reason || "Warning"} tone="wait" track={`events/${e.namespace}/${e.name}/${c.count}`} />
                {kind && c.object_name ? (
                  <Link
                    data-strip
                    to="/envs/$env/k8s/$kind/$ns/$name"
                    params={{ env, kind, ns: c.object_namespace || "-", name: c.object_name }}
                    search={{ tab: "events" }}
                    className="truncate font-ident font-medium hover:underline focus-visible:outline-none"
                  >
                    {object}
                  </Link>
                ) : (
                  <span className="truncate font-ident font-medium">{object}</span>
                )}
                <span className="text-xs whitespace-nowrap text-muted-foreground tabular-nums">
                  {count > 1 && `×${count} · `}
                  {formatAgo(Number(c.last_seen ?? 0))}
                </span>
                <span className="col-start-2 col-end-4 truncate text-xs text-muted-foreground" title={c.message}>
                  {c.message}
                </span>
              </li>
            );
          })}
        </ul>
      )}
    </Section>
  );
}

/** The pods using the most CPU or memory right now. */
function BusiestPanel({ env, rows }: { env: string; rows: KubeUsageRow[] }) {
  const [by, setBy] = useState<"cpu" | "memory">("cpu");
  const top = [...rows].sort((a, b) => b[by] - a[by]).slice(0, BUSIEST_SHOWN);
  const peak = top[0]?.[by] || 1;
  return (
    <Section
      title="Busiest pods"
      icon={Flame}
      aside={
        // 32px targets; the negative margin keeps the header as tall as its neighbours'.
        <span role="group" aria-label="Rank by" className="-my-2 -mr-1.5 flex gap-0.5">
          {(["cpu", "memory"] as const).map((k) => (
            <button
              key={k}
              type="button"
              aria-pressed={by === k}
              onClick={() => setBy(k)}
              className="inline-flex h-8 items-center rounded-sm px-2.5 transition-colors outline-none hover:text-foreground focus-visible:ring-[3px] focus-visible:ring-ring/35 aria-pressed:bg-accent aria-pressed:text-foreground"
            >
              {k === "cpu" ? "CPU" : "Memory"}
            </button>
          ))}
        </span>
      }
      flush
    >
      {top.length === 0 ? (
        <p className="px-4 py-3 text-sm text-muted-foreground">No pods are reporting usage.</p>
      ) : (
        <ul aria-label={`Busiest pods by ${by === "cpu" ? "CPU" : "memory"}`} onKeyDown={walk} className="font-strip text-sm">
          {top.map((r) => (
            <li
              key={`${r.namespace}/${r.name}`}
              className="grid grid-cols-[minmax(0,1fr)_4.5rem] items-center gap-x-3 gap-y-0.5 border-b px-4 py-2 last:border-b-0 strip-focus"
            >
              <Link
                data-strip
                to="/envs/$env/k8s/$kind/$ns/$name"
                params={{ env, kind: "pods", ns: r.namespace ?? "-", name: r.name }}
                className="truncate font-ident font-medium hover:underline focus-visible:outline-none"
              >
                {r.name}
              </Link>
              <Meter
                plain
                className="text-xs"
                value={by === "cpu" ? formatCpu(r.cpu) : formatMemory(r.memory)}
                share={r[by] / peak}
                title={`${formatCpu(r.cpu)} CPU · ${formatMemory(r.memory)} memory`}
              />
              <span className="truncate font-ident text-xs text-muted-foreground">{r.namespace}</span>
            </li>
          ))}
        </ul>
      )}
    </Section>
  );
}

const COUNTED: readonly KubeKind[] = ["services", "ingresses", "configmaps", "secrets", "namespaces"];

/** Counts of everything else, each a door to its list. */
function InventoryPanel({ env, ns, helm }: { env: string; ns?: string; helm: UseQueryResult<HelmRelease[]> }) {
  // One request for every count, from metadata only: no Secret data or
  // ConfigMap bodies cross the wire to draw five numbers.
  const counts = useQuery(kubeCountsQuery(env, COUNTED, ns));
  const count = (kind: KubeKind, scoped = true) => <KindCount env={env} kind={kind} ns={scoped ? ns : undefined} counts={counts} />;
  const items: [ReactNode, string, LucideIcon, ReactNode][] = [
    [count("services"), "Services", Waypoints, <KindLink env={env} kind="services" />],
    [count("ingresses"), "Ingresses", Globe, <KindLink env={env} kind="ingresses" />],
    [count("configmaps"), "ConfigMaps", FileCog, <KindLink env={env} kind="configmaps" />],
    [count("secrets"), "Secrets", KeyRound, <KindLink env={env} kind="secrets" />],
    [count("namespaces", false), "Namespaces", FolderTree, <KindLink env={env} kind="namespaces" />],
    [
      helm.data ? helm.data.length : helm.isPending ? "" : "—",
      "Helm releases",
      Package,
      <Link to="/envs/$env/k8s/helm" params={{ env }} className="absolute inset-0" aria-label="Helm releases" />,
    ],
  ];
  return (
    <Section title="Resources" icon={FolderTree} aside={ns ? <span className="font-ident">{ns}</span> : "All namespaces"} flush>
      <ul className="grid grid-cols-2 text-sm">
        {items.map(([n, label, Icon, link]) => (
          <li
            key={label}
            className="relative flex items-center gap-2.5 border-b px-4 py-2.5 text-muted-foreground transition-colors odd:border-r hover:bg-accent hover:text-foreground [&:nth-last-child(-n+2)]:border-b-0"
          >
            <Icon aria-hidden className="size-4 shrink-0" strokeWidth={1.75} />
            <span className="min-w-0 truncate">{label}</span>
            <span className="font-callsign ml-auto text-base font-semibold text-foreground tabular-nums">{n}</span>
            {link}
          </li>
        ))}
      </ul>
    </Section>
  );
}

function KindCount({ env, kind, ns, counts }: { env: string; kind: KubeKind; ns?: string; counts: UseQueryResult<Record<string, KubeCount>> }) {
  // A list already in the cache (its page was open) is live and exact; prefer it.
  const cached = useQuery({ ...kubeListQuery(env, kind, ns), enabled: false }).data;
  if (cached && !cached.truncated) return <>{cached.rows.length}</>;
  const c = counts.data?.[kind];
  if (c?.count != null) return <>{c.count}</>;
  const why = c?.error === "forbidden" ? "No permission" : c?.error ? c.error : counts.error ? errorMessage(counts.error) : undefined;
  return <span title={why}>{counts.isPending ? "" : "—"}</span>;
}

function KindLink({ env, kind }: { env: string; kind: KubeKind }) {
  return <Link to="/envs/$env/k8s/$kind" params={{ env, kind }} className="absolute inset-0 outline-none focus-visible:ring-[3px] focus-visible:ring-ring/35 focus-visible:ring-inset" aria-label={KIND_LABEL[kind]} />;
}

/** A panel of the board: a titled strip header over its content. */
function Section({ title, icon: Icon, aside, flush, children }: { title: string; icon: LucideIcon; aside?: ReactNode; flush?: boolean; children: ReactNode }) {
  const id = useId();
  return (
    <section aria-labelledby={id} className="grid content-start overflow-hidden rounded-lg border bg-card">
      <div className="flex flex-wrap items-center justify-between gap-x-4 gap-y-1 border-b px-4 py-2.5">
        <h2 id={id} className="flex items-center gap-2 text-sm font-semibold">
          <Icon aria-hidden className="size-4 text-muted-foreground" strokeWidth={1.75} />
          {title}
        </h2>
        {aside && <span className="text-xs text-muted-foreground">{aside}</span>}
      </div>
      <div className={flush ? undefined : "p-4"}>{children}</div>
    </section>
  );
}

function Loading({ label, bare }: { label: string; bare?: boolean }) {
  const bars = (
    <div role="status" className="grid gap-2">
      <span className="sr-only">{label}…</span>
      {[72, 58, 84].map((w) => (
        <div key={w} aria-hidden className="h-4 rounded-xs bg-muted motion-safe:animate-pulse" style={{ width: `${w}%` }} />
      ))}
    </div>
  );
  return bare ? bars : <div className="rounded-lg border bg-card p-4">{bars}</div>;
}

