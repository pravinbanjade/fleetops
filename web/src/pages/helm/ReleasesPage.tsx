import { useQuery } from "@tanstack/react-query";
import { Link, getRouteApi } from "@tanstack/react-router";
import { lazy, Suspense, useMemo, useState } from "react";
import type { HelmRelease } from "@/api/gen/types.gen";
import { useEnvAccess } from "@/api/access";
import { helmCapabilitiesQuery, helmReleasesQuery } from "@/api/queries";
import { Button } from "@/components/ui/button";
import { DataTable, columns } from "@/components/DataTable";
import { Page } from "@/components/Page";
import { formatAgo } from "@/lib/format";
import { NamespacePicker, useNamespace } from "@/pages/kube/NamespacePicker";
import { TopBarActions } from "@/components/topBarSlot";
import { HelmStatus } from "./HelmStatus";

const HelmDeployDialog = lazy(() => import("./HelmDeployDialog"));

const route = getRouteApi("/authed/shell/envs/$env/k8s/helm");

function releaseColumns(env: string) {
  const col = columns<HelmRelease>();
  return [
    col.accessor("name", {
      header: "Name",
      enableHiding: false,
      cell: (c) => (
        <Link
          to="/envs/$env/k8s/helm/$ns/$name"
          params={{ env, ns: c.row.original.namespace, name: c.getValue() }}
          className="font-ident font-medium hover:underline"
        >
          {c.getValue()}
        </Link>
      ),
    }),
    col.accessor("namespace", {
      header: "Namespace",
      cell: (c) => <span className="font-ident text-muted-foreground">{c.getValue()}</span>,
    }),
    col.accessor("status", {
      header: "Status",
      cell: (c) => <HelmStatus status={c.getValue()} error={c.row.original.error} />,
    }),
    col.accessor((r) => (r.chart ? `${r.chart}-${r.chart_version}` : ""), { id: "chart", header: "Chart" }),
    col.accessor("app_version", { header: "App version" }),
    col.accessor("revision", { header: "Revision" }),
    col.accessor("updated", { header: "Updated", cell: (c) => (c.getValue() ? formatAgo(c.getValue()) : "") }),
  ];
}

export function ReleasesPage() {
  const { env } = route.useParams();
  const [ns, setNs] = useNamespace(env);
  const list = useQuery(helmReleasesQuery(env, ns || undefined));
  const cols = useMemo(() => releaseColumns(env), [env]);
  const caps = useQuery(helmCapabilitiesQuery(env));
  const { canWrite } = useEnvAccess(env);
  const [installing, setInstalling] = useState(false);
  return (
    <Page title="Helm releases">
      <TopBarActions>
        <NamespacePicker env={env} value={ns} onChange={setNs} />
        {canWrite && caps.data?.install && (
          <Button size="sm" onClick={() => setInstalling(true)}>
            Install chart
          </Button>
        )}
      </TopBarActions>
      {installing && (
        <Suspense fallback={null}>
          <HelmDeployDialog env={env} target={{ kind: "install", ns: ns || "default" }} onClose={() => setInstalling(false)} />
        </Suspense>
      )}
      <DataTable
        label="Helm releases"
        data={list.data}
        columns={cols}
        getRowId={(r) => `${r.namespace}/${r.name}`}
        error={list.error}
        empty={`No Helm releases in ${ns ? `namespace ${ns}` : "all namespaces"}`}
      />
    </Page>
  );
}
