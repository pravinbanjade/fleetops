import { useQuery } from "@tanstack/react-query";
import { Link, getRouteApi } from "@tanstack/react-router";
import { lazy, Suspense, useMemo, useState } from "react";
import { Button } from "@/components/ui/button";
import type { KubeKind, KubeRow } from "@/api/gen/types.gen";
import { ApiError } from "@/api/client";
import { kubeListQuery } from "@/api/queries";
import { DataTable, columns } from "@/components/DataTable";
import { Page } from "@/components/Page";
import { formatAgo } from "@/lib/format";
import { KubeHealth } from "./KubeHealth";
import { CLUSTER_SCOPED, KIND_COLUMNS, KIND_LABEL, STATUS_HEADER, WORKLOADS, asKind } from "./kinds";
import { LivePaused } from "./LivePaused";
import { NamespacePicker, useNamespace } from "./NamespacePicker";
import { useKubeWatch } from "@/live/kubeWatch";

const route = getRouteApi("/authed/shell/envs/$env/k8s/$kind");
const YamlDialog = lazy(() => import("@/components/yaml-editor/YamlDialog"));

function kindColumns(env: string, kind: KubeKind) {
  const col = columns<KubeRow>();
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
    }),
    ...KIND_COLUMNS[kind].map(([key, header]) => col.accessor((r) => r.columns[key] ?? "", { id: key, header })),
    col.accessor("created", { header: "Age", cell: (c) => formatAgo(c.getValue()) }),
  ];
}

export function KubeListPage() {
  const { env, kind: raw } = route.useParams();
  const kind = asKind(raw) ?? "pods";
  const [ns, setNs] = useNamespace(env);
  const scoped = !CLUSTER_SCOPED.includes(kind);
  const list = useQuery(kubeListQuery(env, kind, scoped ? ns : undefined));
  const live = useKubeWatch(env, kind, scoped ? ns : undefined);
  const [creating, setCreating] = useState(false);
  const cols = useMemo(() => kindColumns(env, kind), [env, kind]);
  const forbidden = list.error instanceof ApiError && list.error.status === 403;
  const where = ns ? `namespace ${ns}` : "all namespaces";
  return (
    <Page
      title={KIND_LABEL[kind]}
      actions={
        <div className="flex items-center gap-2">
          {scoped && <NamespacePicker env={env} value={ns} onChange={setNs} />}
          {kind !== "nodes" && (
            <Button variant="outline" size="sm" onClick={() => setCreating(true)}>
              Create
            </Button>
          )}
        </div>
      }
    >
      {live.paused && <LivePaused paused={live.paused} onRetry={live.retry} />}
      {creating && (
        <Suspense fallback={null}>
          <YamlDialog env={env} target={{ mode: "create", kind, namespace: ns || "default" }} onClose={() => setCreating(false)} />
        </Suspense>
      )}
      {WORKLOADS.includes(kind) && (
        <nav aria-label="Workload kinds" className="mb-3 flex flex-wrap gap-1">
          {WORKLOADS.map((k) => (
            <Link
              key={k}
              to="/envs/$env/k8s/$kind"
              params={{ env, kind: k }}
              className="rounded-md px-2.5 py-1 text-sm hover:bg-accent data-[status=active]:bg-accent data-[status=active]:font-medium"
            >
              {KIND_LABEL[k]}
            </Link>
          ))}
        </nav>
      )}
      {list.data?.truncated && (
        <p role="status" className="mb-3 rounded-md border border-state-wait/40 bg-state-wait/10 px-3 py-2 text-sm">
          Showing the first 5,000. Pick a namespace to narrow the list.
        </p>
      )}
      <DataTable
        label={KIND_LABEL[kind]}
        data={forbidden ? [] : list.data?.rows}
        columns={cols}
        getRowId={(r) => `${r.namespace ?? "-"}/${r.name}`}
        error={forbidden ? undefined : list.error}
        empty={forbidden ? `No permission to list ${KIND_LABEL[kind]} in ${scoped ? where : "this cluster"}` : `No ${KIND_LABEL[kind].toLowerCase()} in ${scoped ? where : "this cluster"}`}
      />
    </Page>
  );
}
