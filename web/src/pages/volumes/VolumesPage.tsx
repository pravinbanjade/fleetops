import { Eraser, Plus, Trash2 } from "lucide-react";
import { useNow, useNowRow } from "@/components/shell/now";
import { ObjectNow } from "@/components/shell/ObjectNow";
import { useQuery } from "@tanstack/react-query";
import { Link, useParams } from "@tanstack/react-router";
import { useMemo, useState } from "react";
import { containersQuery, volumesQuery } from "@/api/queries";
import type { ContainerSummary, VolumeSummary } from "@/api/gen/types.gen";
import { columns, DataTable, type Column } from "@/components/DataTable";
import { Page } from "@/components/Page";
import { useEnvAccess } from "@/api/access";
import { Button } from "@/components/ui/button";
import { volumeUsers } from "@/lib/usage";
import { CreateVolumeDialog, PruneVolumesDialog, RemoveVolumesDialog } from "./dialogs";

type Row = VolumeSummary & { users: ContainerSummary[] };
const col = columns<Row>();

function volumeColumns(env: string): Column<Row>[] {
  return [
    col.accessor("name", {
      header: "Name",
      enableHiding: false,
      cell: (c) => (
        <Link
          to="/envs/$env/volumes/$name"
          params={{ env, name: c.getValue() }}
          className="block max-w-80 truncate font-ident font-medium hover:underline"
        >
          {c.getValue()}
        </Link>
      ),
    }),
    col.accessor("driver", { header: "Driver" }),
    col.accessor((v) => v.users.map((u) => u.name).join(", "), {
      id: "users",
      header: "Used by",
      cell: (c) =>
        c.row.original.users.length ? (
          <span className="flex flex-wrap gap-x-2">
            {c.row.original.users.map((u) => (
              <Link key={u.id} to="/envs/$env/containers/$id" params={{ env, id: u.id }} className="hover:underline">
                {u.name}
              </Link>
            ))}
          </span>
        ) : (
          <span className="text-muted-foreground">{c.row.original.in_use ? "in use" : "unused"}</span>
        ),
    }),
    col.accessor("mountpoint", {
      header: "Mount point",
      cell: (c) => <span className="block max-w-80 truncate font-mono text-xs">{c.getValue()}</span>,
    }),
    col.accessor((v) => v.created_at ?? "", {
      id: "created",
      header: "Created",
      cell: (c) => (c.getValue() ? new Date(c.getValue()).toLocaleString() : "—"),
    }),
  ];
}

export function VolumesPage() {
  const { env } = useParams({ from: "/authed/shell/envs/$env/volumes" });
  const { can } = useEnvAccess(env);
  const canDelete = can("volumes.delete");
  const list = useQuery(volumesQuery(env));
  const containers = useQuery(containersQuery(env));
  const cols = useMemo(() => volumeColumns(env), [env]);
  const rows = useMemo(() => {
    if (!list.data) return undefined;
    const users = volumeUsers(containers.data);
    return list.data.map((v) => ({ ...v, users: users.get(v.name) ?? [] }));
  }, [list.data, containers.data]);
  const now = useNow();
  const active = useNowRow(rows, (v: Row) => v.name);
  const [removing, setRemoving] = useState<string[] | null>(null);
  const [pruning, setPruning] = useState(false);
  const [creating, setCreating] = useState(false);
  return (
    <Page
      title="Volumes"
      actions={
        <>
          {canDelete && (
            <Button variant="outline" onClick={() => setPruning(true)}>
              <Eraser aria-hidden />
              Prune
            </Button>
          )}
          {can("volumes.create") && <Button onClick={() => setCreating(true)}>
<Plus aria-hidden />Create volume</Button>}
        </>
      }
    >
      {active && <VolumeNow key={active.name} env={env} volume={active} />}
      <DataTable
        label="Volumes"
        data={rows}
        error={list.error}
        columns={cols}
        onActivate={(r) => now.open(((v: Row) => v.name)(r))} activeId={active ? ((v: Row) => v.name)(active) : undefined} getRowId={(v) => v.name}
        empty="No volumes in this environment."
        initialSorting={[{ id: "name", desc: false }]}
        initialHidden={["mountpoint"]}
        selectable={canDelete}
        actions={
          canDelete
            ? (selected, clear) => (
                <Button
                  variant="destructive"
                  size="sm"
                  onClick={() => {
                    setRemoving(selected.map((v) => v.name));
                    clear();
                  }}
                >
                  <Trash2 aria-hidden />
                  Remove
                </Button>
              )
            : undefined
        }
      />
      {removing && <RemoveVolumesDialog env={env} names={removing} onDone={() => setRemoving(null)} />}
      {pruning && <PruneVolumesDialog env={env} onDone={() => setPruning(false)} />}
      {creating && <CreateVolumeDialog env={env} onClose={() => setCreating(false)} />}
    </Page>
  );
}

/** A volume in the panel: its driver and mountpoint, and the containers that mount it. */
function VolumeNow({ env, volume }: { env: string; volume: Row }) {
  return (
    <ObjectNow
      title={<span className="font-ident">{volume.name}</span>}
      subtitle={<span className="font-ident">{volume.mountpoint}</span>}
      facts={[
        { label: "Driver", value: volume.driver },
        { label: "Containers", value: <span className="font-figure">{volume.users.length}</span> },
      ]}
      list={{
        title: "Mounted by",
        empty: "No container mounts this volume.",
        rows: volume.users.map((c) => ({ key: c.id, label: c.name, aside: c.state, link: { to: "/envs/$env/containers/$id", params: { env, id: c.id } } })),
      }}
      open={{ to: "/envs/$env/volumes/$name", params: { env, name: volume.name } }}
    />
  );
}
