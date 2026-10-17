import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, getRouteApi, useNavigate } from "@tanstack/react-router";
import { FileCode2, ListTree, Trash2 } from "lucide-react";
import { lazy, Suspense, useMemo, useState } from "react";
import type { ApiKind, KubeRow } from "@/api/gen/types.gen";
import { deleteGenericKube } from "@/api/gen/sdk.gen";
import { ApiError } from "@/api/client";
import { useEnvAccess } from "@/api/access";
import { genericListQuery } from "@/api/queries";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { DataTable, columns } from "@/components/DataTable";
import { Page } from "@/components/Page";
import { QuickButtons, type QuickItem } from "@/components/RowMenu";
import { formatAgo } from "@/lib/format";
import { useKubeWatch } from "@/live/kubeWatch";
import { KubeHealth } from "../KubeHealth";
import { HEALTH_BAYS, HEALTH_RANK } from "../kinds";
import { LivePaused } from "../LivePaused";
import { NamespacePicker, useNamespace } from "../NamespacePicker";
import { TopBarActions } from "@/components/topBarSlot";
import { refKind, type Ref } from "./ref";
import { useDiscoveredKind } from "./useDiscoveredKind";

const route = getRouteApi("/authed/shell/envs/$env/k8s/r/$group/$version/$resource");
const YamlDialog = lazy(() => import("@/components/yaml-editor/YamlDialog"));

/** A printer column's cell: dates as ages, the rest as the cluster wrote them. */
export const printed = (type: string, v: string | undefined) => (type === "date" && Number(v) ? formatAgo(Number(v)) : v || "");

function genericColumns(env: string, r: Ref, k: ApiKind) {
  const col = columns<KubeRow>();
  return [
    col.accessor("name", {
      header: "Name",
      enableHiding: false,
      cell: (c) => (
        <Link
          to="/envs/$env/k8s/r/$group/$version/$resource/$ns/$name"
          params={{ env, ...r, ns: c.row.original.namespace ?? "-", name: c.getValue() }}
          className="font-ident font-medium hover:underline"
        >
          {c.getValue()}
        </Link>
      ),
    }),
    ...(k.namespaced
      ? [
          col.accessor((x) => x.namespace ?? "", {
            id: "namespace",
            header: "Namespace",
            cell: (c) => <span className="font-ident text-muted-foreground">{c.getValue()}</span>,
          }),
        ]
      : []),
    col.accessor("status", {
      header: "Status",
      cell: (c) => <KubeHealth health={c.row.original.health} label={c.getValue() || "—"} />,
      sortFn: (a, b) => HEALTH_RANK[a.original.health] - HEALTH_RANK[b.original.health],
    }),
    ...k.columns.map((p) =>
      col.accessor((x) => x.columns[p.name] ?? "", {
        id: `c:${p.name}`,
        header: p.name,
        cell: (c) => printed(p.type, c.getValue()),
      }),
    ),
    col.accessor("created", { header: "Age", cell: (c) => formatAgo(c.getValue()) }),
  ];
}

/** A discovered kind's live list (spec 3g §2.5). */
export function GenericListPage() {
  const { env, group, version, resource } = route.useParams();
  const r = useMemo<Ref>(() => ({ group, version, resource }), [group, version, resource]);
  const qc = useQueryClient();
  const { kind: k, discovery } = useDiscoveredKind(env, r);
  const [ns, setNs] = useNamespace(env);
  const { can } = useEnvAccess(env);
  const scoped = k?.namespaced ?? true;
  const live = useKubeWatch(env, refKind(r), scoped ? ns : undefined);
  const list = useQuery({ ...genericListQuery(env, r, scoped ? ns : undefined), enabled: !live.synced });
  const [doomed, setDoomed] = useState<{ rows: KubeRow[]; clear: () => void } | null>(null);
  const [editing, setEditing] = useState<{ ns: string; name: string } | null>(null);
  const navigate = useNavigate();
  const canEdit = can("k8s-other.operate") && !!k?.verbs.includes("update");
  const canDelete = can("k8s-other.delete") && !!k?.verbs.includes("delete");
  /** What can be done to `of`: its events and YAML need exactly one; Delete takes any number. */
  const quick = (of: KubeRow[], clear: () => void): QuickItem[] => {
    const out: QuickItem[] = [];
    if (of.length === 1) {
      const at = { ns: of[0].namespace ?? "-", name: of[0].name };
      out.push({
        id: "events",
        label: "Events",
        icon: ListTree,
        onSelect: () => void navigate({ to: "/envs/$env/k8s/r/$group/$version/$resource/$ns/$name", params: { env, ...r, ...at }, search: { tab: "events" } }),
      });
      if (canEdit) out.push({ id: "edit", label: "Edit YAML", icon: FileCode2, onSelect: () => setEditing(at) });
    }
    if (canDelete) out.push({ id: "delete", label: "Delete", icon: Trash2, count: of.length, danger: true, onSelect: () => setDoomed({ rows: of, clear }) });
    return out;
  };
  const cols = useMemo(() => (k ? genericColumns(env, r, k) : []), [env, r, k]);
  const gone = list.error instanceof ApiError && list.error.status === 404;
  const forbidden = list.error instanceof ApiError && list.error.status === 403;
  const label = k?.kind ?? resource;
  if (gone || (discovery.isSuccess && !k && !list.isPending && !list.data))
    return (
      <Page title={resource}>
        <p role="status" className="text-sm">
          This resource type no longer exists in this cluster.
        </p>
      </Page>
    );
  const where = scoped ? (ns ? `namespace ${ns}` : "all namespaces") : "this cluster";
  return (
    <Page
      title={label}
      meta={k && <span className="font-ident">{`${k.api.group || "core"}/${k.api.version}`}</span>}
    >
      {scoped && (
        <TopBarActions>
          <NamespacePicker env={env} value={ns} onChange={setNs} />
        </TopBarActions>
      )}
      {live.paused && <LivePaused paused={live.paused} onRetry={live.retry} />}
      {editing && k && (
        <Suspense fallback={null}>
          <YamlDialog env={env} target={{ mode: "edit-generic", ref: r, kind: k.kind, ...editing }} onClose={() => setEditing(null)} />
        </Suspense>
      )}
      {doomed && (
        <ConfirmDialog
          open
          onOpenChange={(o) => !o && setDoomed(null)}
          title={`Delete ${doomed.rows.length === 1 ? doomed.rows[0].name : `${doomed.rows.length} ${label}`}?`}
          description="This cannot be undone."
          confirmLabel="Delete"
          destructive
          onConfirm={async () => {
            // One at a time, stopping at the first failure, as curated bulk actions do.
            for (const row of doomed.rows) {
              await deleteGenericKube({ path: { env, ...r, ns: row.namespace ?? "-", name: row.name }, throwOnError: true });
            }
            await qc.invalidateQueries({ queryKey: ["env", env, "k8s"] });
            doomed.clear();
            setDoomed(null);
          }}
        />
      )}
      {list.data?.truncated && (
        <p role="status" className="mb-3 rounded-md border border-state-wait/40 bg-state-wait/10 px-3 py-2 text-sm">
          Showing the first 5,000. Pick a namespace to narrow the list.
        </p>
      )}
      <DataTable
        label={label}
        data={forbidden ? [] : list.data?.rows}
        columns={cols}
        getRowId={(x) => `${x.namespace ?? "-"}/${x.name}`}
        selectable={canDelete}
        actions={(selected, clear) => <QuickButtons items={quick(selected, clear)} />}
        rowMenu={{ label: (x) => x.name, items: (x) => quick([x], () => {}) }}
        initialSorting={[
          { id: "status", desc: false },
          { id: "name", desc: false },
        ]}
        bays={{ column: "status", of: (x: KubeRow) => HEALTH_BAYS[HEALTH_RANK[x.health]] }}
        phoneColumns={["name", "status"]}
        error={forbidden ? undefined : list.error}
        empty={forbidden ? `This cluster's credentials can't list ${label}.` : `No ${label} in ${where}`}
      />
    </Page>
  );
}
