import { useQuery } from "@tanstack/react-query";
import { Link, useParams } from "@tanstack/react-router";
import { useMemo, useState } from "react";
import { networksQuery } from "@/api/queries";
import type { NetworkSummary } from "@/api/gen/types.gen";
import { columns, DataTable, type Column } from "@/components/DataTable";
import { Page } from "@/components/Page";
import { Badge } from "@/components/ui/badge";
import { useEnvAccess } from "@/api/access";
import { Button } from "@/components/ui/button";
import { CreateNetworkDialog, PruneNetworksDialog, RemoveNetworksDialog } from "./dialogs";

const col = columns<NetworkSummary>();

function networkColumns(env: string): Column<NetworkSummary>[] {
  return [
    col.accessor("name", {
      header: "Name",
      enableHiding: false,
      cell: (c) => (
        <Link
          to="/envs/$env/networks/$id"
          params={{ env, id: c.row.original.id }}
          className="font-ident font-medium hover:underline"
        >
          {c.getValue()}
        </Link>
      ),
    }),
    col.accessor("driver", { header: "Driver" }),
    col.accessor("scope", { header: "Scope" }),
    col.accessor((n) => n.subnet ?? "", { id: "subnet", header: "Subnet", cell: (c) => c.getValue() || "—" }),
    col.accessor((n) => n.gateway ?? "", { id: "gateway", header: "Gateway", cell: (c) => c.getValue() || "—" }),
    col.accessor((n) => [n.internal && "internal", n.attachable && "attachable"].filter(Boolean).join(" "), {
      id: "flags",
      header: "Flags",
      enableSorting: false,
      cell: (c) => (
        <span className="flex gap-1">
          {c.row.original.internal && <Badge variant="outline">internal</Badge>}
          {c.row.original.attachable && <Badge variant="outline">attachable</Badge>}
        </span>
      ),
    }),
  ];
}

export function NetworksPage() {
  const { env } = useParams({ from: "/authed/shell/envs/$env/networks" });
  const { can } = useEnvAccess(env);
  const canDelete = can("networks.delete");
  const list = useQuery(networksQuery(env));
  const cols = useMemo(() => networkColumns(env), [env]);
  const [removing, setRemoving] = useState<NetworkSummary[] | null>(null);
  const [pruning, setPruning] = useState(false);
  const [creating, setCreating] = useState(false);
  return (
    <Page
      title="Networks"
      actions={
        <>
          {canDelete && (
            <Button variant="outline" onClick={() => setPruning(true)}>
              Prune
            </Button>
          )}
          {can("networks.create") && <Button onClick={() => setCreating(true)}>Create network</Button>}
        </>
      }
    >
      <DataTable
        label="Networks"
        data={list.data}
        error={list.error}
        columns={cols}
        getRowId={(n) => n.id}
        empty="No networks in this environment."
        initialSorting={[{ id: "name", desc: false }]}
        selectable={canDelete}
        actions={
          canDelete
            ? (selected, clear) => (
                <Button
                  variant="destructive"
                  size="sm"
                  onClick={() => {
                    setRemoving(selected);
                    clear();
                  }}
                >
                  Remove
                </Button>
              )
            : undefined
        }
      />
      {removing && (
        <RemoveNetworksDialog
          env={env}
          networks={removing.map((n) => ({ id: n.id, name: n.name }))}
          onDone={() => setRemoving(null)}
        />
      )}
      {pruning && <PruneNetworksDialog env={env} onDone={() => setPruning(false)} />}
      {creating && <CreateNetworkDialog env={env} onClose={() => setCreating(false)} />}
    </Page>
  );
}
