import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, getRouteApi } from "@tanstack/react-router";
import { useMemo, useState } from "react";
import type { ApiKind, KubeRow } from "@/api/gen/types.gen";
import { deleteGenericKube } from "@/api/gen/sdk.gen";
import { ApiError } from "@/api/client";
import { useEnvAccess } from "@/api/access";
import { discoveryQuery, genericListQuery } from "@/api/queries";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { DataTable, columns } from "@/components/DataTable";
import { Page } from "@/components/Page";
import { Button } from "@/components/ui/button";
import { formatAgo } from "@/lib/format";
import { useKubeWatch } from "@/live/kubeWatch";
import { KubeHealth } from "../KubeHealth";
import { HEALTH_BAYS, HEALTH_RANK } from "../kinds";
import { LivePaused } from "../LivePaused";
import { NamespacePicker, useNamespace } from "../NamespacePicker";
import { findKind, refKind, type Ref } from "./ref";

const route = getRouteApi("/authed/shell/envs/$env/k8s/r/$group/$version/$resource");

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
  const discovery = useQuery(discoveryQuery(env));
  const k = findKind(discovery.data, r);
  const [ns, setNs] = useNamespace(env);
  const { canWrite } = useEnvAccess(env);
  const scoped = k?.namespaced ?? true;
  const live = useKubeWatch(env, refKind(r), scoped ? ns : undefined);
  const list = useQuery({ ...genericListQuery(env, r, scoped ? ns : undefined), enabled: !live.synced });
  const [doomed, setDoomed] = useState<{ rows: KubeRow[]; clear: () => void } | null>(null);
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
      actions={scoped ? <NamespacePicker env={env} value={ns} onChange={setNs} /> : undefined}
    >
      {live.paused && <LivePaused paused={live.paused} onRetry={live.retry} />}
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
        selectable={canWrite && !!k?.verbs.includes("delete")}
        actions={(selected, clear) => (
          <Button
            variant="outline"
            size="sm"
            className="text-state-fail-text hover:text-state-fail-text"
            onClick={() => setDoomed({ rows: selected, clear })}
          >
            Delete{selected.length > 1 ? ` ${selected.length}` : ""}
          </Button>
        )}
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
