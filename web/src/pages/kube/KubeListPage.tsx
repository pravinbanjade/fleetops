import { useQuery } from "@tanstack/react-query";
import { Link, getRouteApi, useNavigate } from "@tanstack/react-router";
import { FileCode2, ListTree, ScrollText, TerminalSquare } from "lucide-react";
import { lazy, Suspense, useMemo, useState } from "react";
import { useEnvAccess } from "@/api/access";
import { kubePerm } from "./perms";
import { Button } from "@/components/ui/button";
import type { KubeKind, KubeRow, KubeUsageRow } from "@/api/gen/types.gen";
import { ApiError } from "@/api/client";
import { kubeListQuery } from "@/api/queries";
import { DataTable, columns } from "@/components/DataTable";
import { Page } from "@/components/Page";
import { StateBadge } from "@/components/StateBadge";
import { formatAgo, formatCores, formatCpu, formatMemory, formatPercent } from "@/lib/format";
import { KubeHealth } from "./KubeHealth";
import { CLUSTER_SCOPED, HEALTH_BAYS, HEALTH_KINDS, HEALTH_RANK, KIND_COLUMNS, KIND_LABEL, LINK_COLUMNS, NO_CREATE, READ_ONLY, STATUS_HEADER, asKind } from "./kinds";
import { ActDialog, actIcon, actLabel, type Act, type ActTarget } from "./Act";
import { kubeQuick, type KubeQuick } from "./quick";
import { QuickButtons, type QuickItem } from "@/components/RowMenu";
import { Meter } from "./Meter";
import { ContainerSquares, readyCount } from "./ContainerSquares";
import { share, useKubeUsage, usageKey } from "./usage";
import { LivePaused } from "./LivePaused";
import { NamespacePicker, useNamespace } from "./NamespacePicker";
import { TopBarActions } from "@/components/topBarSlot";
import { useKubeWatch } from "@/live/kubeWatch";

const route = getRouteApi("/authed/shell/envs/$env/k8s/$kind");
const YamlDialog = lazy(() => import("@/components/yaml-editor/YamlDialog"));

/** A row with its CPU and memory now, on the kinds metrics-server reports. */
type Row = KubeRow & { usage?: KubeUsageRow };

const OPEN = {
  logs: { label: "Logs", icon: ScrollText },
  terminal: { label: "Terminal", icon: TerminalSquare },
  events: { label: "Events", icon: ListTree },
} as const;

const byNumber = (f: (r: Row) => number) => (a: { original: Row }, b: { original: Row }) => f(a.original) - f(b.original);

function usageColumns(kind: KubeKind) {
  const col = columns<Row>();
  if (kind === "nodes") {
    const meter = (used: number, alloc: number | null | undefined, title: string) => {
      const s = share(used, alloc);
      return <Meter className="w-20" value={s === undefined ? title : formatPercent(s * 100)} share={s} title={title} />;
    };
    return [
      col.accessor((r) => (r.usage ? (share(r.usage.cpu, r.usage.cpu_allocatable) ?? 0) : -1), {
        id: "cpu",
        header: "CPU",
        cell: (c) => {
          const u = c.row.original.usage;
          if (!u) return <span className="text-muted-foreground">—</span>;
          return meter(u.cpu, u.cpu_allocatable, `${formatCpu(u.cpu)}${u.cpu_allocatable ? ` of ${formatCores(u.cpu_allocatable)} cores` : ""}`);
        },
      }),
      col.accessor((r) => (r.usage ? (share(r.usage.memory, r.usage.memory_allocatable) ?? 0) : -1), {
        id: "memory",
        header: "Memory",
        cell: (c) => {
          const u = c.row.original.usage;
          if (!u) return <span className="text-muted-foreground">—</span>;
          return meter(u.memory, u.memory_allocatable, `${formatMemory(u.memory)}${u.memory_allocatable ? ` of ${formatMemory(u.memory_allocatable)}` : ""}`);
        },
      }),
    ];
  }
  return [
    col.accessor((r) => r.usage?.cpu ?? -1, {
      id: "cpu",
      header: "CPU",
      cell: (c) => <span className="tabular-nums">{c.row.original.usage ? formatCpu(c.getValue()) : "—"}</span>,
    }),
    col.accessor((r) => r.usage?.memory ?? -1, {
      id: "memory",
      header: "Memory",
      cell: (c) => <span className="tabular-nums">{c.row.original.usage ? formatMemory(c.getValue()) : "—"}</span>,
    }),
  ];
}

/** What happened, to what, how often and when: the newest warnings first. */
function eventColumns(env: string) {
  const col = columns<Row>();
  return [
    col.accessor("status", {
      header: "Type",
      enableHiding: false,
      cell: (c) => <StateBadge state={c.getValue()} tone={c.getValue() === "Warning" ? "wait" : "idle"} />,
      sortFn: byNumber((r) => (r.status === "Warning" ? 0 : 1)),
    }),
    col.accessor((r) => r.columns.reason ?? "", {
      id: "reason",
      header: "Reason",
      cell: (c) => <span className="font-ident">{c.getValue()}</span>,
    }),
    col.accessor((r) => `${r.columns.object_kind ?? ""}/${r.columns.object_name ?? ""}`, {
      id: "object",
      header: "Object",
      cell: (c) => {
        const k = asKind(c.row.original.columns.object_link ?? "");
        const name = c.row.original.columns.object_name ?? "";
        const label = (
          <>
            <span className="text-muted-foreground">{c.row.original.columns.object_kind}/</span>
            {name}
          </>
        );
        return k && name ? (
          <Link
            to="/envs/$env/k8s/$kind/$ns/$name"
            params={{ env, kind: k, ns: c.row.original.columns.object_namespace || "-", name }}
            className="font-ident font-medium hover:underline"
          >
            {label}
          </Link>
        ) : (
          <span className="font-ident">{label}</span>
        );
      },
    }),
    col.accessor((r) => r.namespace ?? "", {
      id: "namespace",
      header: "Namespace",
      cell: (c) => <span className="font-ident text-muted-foreground">{c.getValue()}</span>,
    }),
    col.accessor((r) => r.columns.message ?? "", {
      id: "message",
      header: "Message",
      cell: (c) => {
        const r = c.row.original.columns;
        return (
          <span className="grid gap-0.5">
            {/* On a phone the object rides above the message: the table keeps two columns. */}
            <span className="font-ident text-xs text-muted-foreground sm:hidden">
              {r.reason} · {r.object_kind}/{r.object_name}
            </span>
            <span className="line-clamp-2 max-w-[44ch] whitespace-normal">{c.getValue()}</span>
          </span>
        );
      },
    }),
    col.accessor((r) => Number(r.columns.count ?? 1), {
      id: "count",
      header: "Count",
      cell: (c) => <span className="tabular-nums">{c.getValue() > 1 ? `×${c.getValue()}` : ""}</span>,
    }),
    col.accessor((r) => Number(r.columns.last_seen ?? 0), {
      id: "last_seen",
      header: "Last seen",
      cell: (c) => <span title={new Date(c.getValue() * 1000).toLocaleString()}>{formatAgo(c.getValue())}</span>,
    }),
    col.accessor((r) => r.columns.source ?? "", { id: "source", header: "Source" }),
  ];
}

const EVENT_BAYS = { warning: { id: "warning", label: "Warnings" }, normal: { id: "normal", label: "Normal" } };

function kindColumns(env: string, kind: KubeKind, withUsage: boolean) {
  if (kind === "events") return eventColumns(env);
  const col = columns<Row>();
  const scoped = !CLUSTER_SCOPED.includes(kind);
  return [
    col.accessor("name", {
      header: "Name",
      enableHiding: false,
      cell: (c) => (
        <Link
          to="/envs/$env/k8s/$kind/$ns/$name"
          params={{ env, kind, ns: c.row.original.namespace ?? "-", name: c.getValue() }}
          className="font-ident font-medium hover:underline"
        >
          {c.getValue()}
        </Link>
      ),
    }),
    ...(scoped ? [col.accessor((r) => r.namespace ?? "", {
            id: "namespace",
            header: "Namespace",
            cell: (c) => <span className="font-ident text-muted-foreground">{c.getValue()}</span>,
          })] : []),
    col.accessor("status", {
      header: STATUS_HEADER[kind] ?? "Status",
      cell: (c) => <KubeHealth health={c.row.original.health} label={c.getValue()} />,
      ...(HEALTH_KINDS.includes(kind) ? { sortFn: byNumber((r) => HEALTH_RANK[r.health]) } : {}),
    }),
    ...KIND_COLUMNS[kind].map(([key, header]) =>
      key === "containers"
        ? col.accessor((r) => r.columns.containers ?? "", {
            id: key,
            header,
            cell: (c) => <ContainerSquares column={c.getValue()} ready={c.row.original.columns.ready} />,
            sortFn: byNumber((r) => readyCount(r.columns.ready)),
          })
        : col.accessor((r) => r.columns[key] ?? "", {
            id: key,
            header,
            cell: (c) => {
              const v = c.getValue();
              const to = v ? LINK_COLUMNS[kind]?.[key]?.(c.row.original.columns, c.row.original.namespace ?? null) : undefined;
              return to ? (
                <Link to="/envs/$env/k8s/$kind/$ns/$name" params={{ env, kind: to.kind, ns: to.ns, name: v }} className="font-ident hover:underline">
                  {v}
                </Link>
              ) : (
                v
              );
            },
          }),
    ),
    ...(withUsage ? usageColumns(kind) : []),
    col.accessor("created", { header: "Age", cell: (c) => formatAgo(c.getValue()) }),
  ];
}

export function KubeListPage() {
  const { env, kind: raw } = route.useParams();
  const { q } = route.useSearch();
  const kind = asKind(raw) ?? "pods";
  const [ns, setNs] = useNamespace(env);
  const { can } = useEnvAccess(env);
  const scoped = !CLUSTER_SCOPED.includes(kind);
  const live = useKubeWatch(env, kind, scoped ? ns : undefined);
  // Once the watch has its snapshot the cache follows the cluster; a REST
  // refetch (after a write, on focus) would race it with older rows.
  const list = useQuery({ ...kubeListQuery(env, kind, scoped ? ns : undefined), enabled: !live.synced });
  const [creating, setCreating] = useState(false);
  const [bulk, setBulk] = useState<{ act: Act; targets: ActTarget[]; clear: () => void } | null>(null);
  const [editing, setEditing] = useState<{ ns: string; name: string } | null>(null);
  const navigate = useNavigate();
  /** The quick actions for `of` as menu items and bar buttons: opening a tab needs exactly one row. */
  const quickItems = (qs: KubeQuick[], of: KubeRow[], clear: () => void): QuickItem[] =>
    qs.map((q): QuickItem => {
      const first = { ns: of[0].namespace ?? "-", name: of[0].name };
      switch (q.type) {
        case "open":
          return {
            id: q.tab,
            ...OPEN[q.tab],
            onSelect: () => void navigate({ to: "/envs/$env/k8s/$kind/$ns/$name", params: { env, kind, ...first }, search: { tab: q.tab } }),
          };
        case "edit":
          return { id: "edit", label: "Edit YAML", icon: FileCode2, onSelect: () => setEditing(first) };
        case "act":
          return {
            id: q.act,
            label: actLabel(q.act),
            icon: actIcon(q.act),
            count: q.rows.length,
            danger: q.act === "delete",
            onSelect: () => setBulk({ act: q.act, targets: q.rows.map((r) => ({ kind, ns: r.namespace ?? "-", name: r.name, row: r })), clear }),
          };
      }
    });
  const selectable = !READ_ONLY.includes(kind) && (can(kubePerm(kind, "operate")) || can(kubePerm(kind, "delete")));
  const createIn = scoped ? ns || "default" : null;
  const metered = kind === "pods" || kind === "nodes";
  const usage = useKubeUsage(env, kind === "nodes" ? "nodes" : "pods", kind === "pods" ? ns : undefined, metered);
  const withUsage = metered && usage.available;
  const cols = useMemo(() => kindColumns(env, kind, withUsage), [env, kind, withUsage]);
  const rows = useMemo<Row[] | undefined>(
    () => (withUsage ? list.data?.rows.map((r) => ({ ...r, usage: usage.index.get(usageKey(r.namespace, r.name)) })) : list.data?.rows),
    [withUsage, list.data, usage.index],
  );
  const forbidden = list.error instanceof ApiError && list.error.status === 403;
  const where = ns ? `namespace ${ns}` : "all namespaces";
  return (
    <Page
      title={KIND_LABEL[kind]}
      meta={metered && usage.reason ? <span>No CPU or memory figures: {usage.reason}.</span> : undefined}
      actions={
        <>
          {scoped && (
            <TopBarActions>
              <NamespacePicker env={env} value={ns} onChange={setNs} />
            </TopBarActions>
          )}
          {can(kubePerm(kind, "create"), createIn) && can(kubePerm(kind, "operate"), createIn) && !NO_CREATE.includes(kind) && !READ_ONLY.includes(kind) && (
            <Button variant="outline" size="sm" onClick={() => setCreating(true)}>
              Create
            </Button>
          )}
        </>
      }
    >
      {live.paused && <LivePaused paused={live.paused} onRetry={live.retry} />}
      {creating && (
        <Suspense fallback={null}>
          <YamlDialog env={env} target={{ mode: "create", kind, namespace: ns || "default" }} onClose={() => setCreating(false)} />
        </Suspense>
      )}
      {editing && (
        <Suspense fallback={null}>
          <YamlDialog env={env} target={{ mode: "edit", kind, ...editing }} onClose={() => setEditing(null)} />
        </Suspense>
      )}
      {bulk && (
        <ActDialog
          env={env}
          act={bulk.act}
          targets={bulk.targets}
          onClose={() => setBulk(null)}
          onDone={() => {
            bulk.clear();
            setBulk(null);
          }}
        />
      )}
      {list.data?.truncated && (
        <p role="status" className="mb-3 rounded-md border border-state-wait/40 bg-state-wait/10 px-3 py-2 text-sm">
          Showing the first 5,000. Pick a namespace to narrow the list.
        </p>
      )}
      <DataTable
        label={KIND_LABEL[kind]}
        data={forbidden ? [] : rows}
        columns={cols}
        getRowId={(r) => `${r.namespace ?? "-"}/${r.name}`}
        initialFilter={q}
        selectable={selectable}
        actions={(selected, clear) => <QuickButtons items={quickItems(kubeQuick(kind, selected, can), selected, clear)} />}
        rowMenu={
          READ_ONLY.includes(kind)
            ? undefined
            : { label: (r) => r.name, items: (r) => quickItems(kubeQuick(kind, [r], can), [r], () => {}) }
        }
        {...(kind === "events"
          ? {
              initialSorting: [{ id: "status", desc: false }, { id: "last_seen", desc: true }],
              bays: { column: "status", of: (r: Row) => (r.status === "Warning" ? EVENT_BAYS.warning : EVENT_BAYS.normal) },
            }
          : // A pod's squares already say what is wrong: pods stay one list.
            HEALTH_KINDS.includes(kind) && kind !== "pods"
          ? {
              initialSorting: [{ id: "status", desc: false }, { id: "name", desc: false }],
              bays: { column: "status", of: (r: Row) => HEALTH_BAYS[HEALTH_RANK[r.health]] },
            }
          : {})}
        phoneColumns={kind === "events" ? ["status", "message"] : ["name", "status"]}
        error={forbidden ? undefined : list.error}
        empty={forbidden ? `No permission to list ${KIND_LABEL[kind]} in ${scoped ? where : "this cluster"}` : `No ${KIND_LABEL[kind].toLowerCase()} in ${scoped ? where : "this cluster"}`}
      />
    </Page>
  );
}
