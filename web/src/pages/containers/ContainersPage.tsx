import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useParams } from "@tanstack/react-router";
import { useState } from "react";
import { toast } from "sonner";
import { containersQuery, keys } from "@/api/queries";
import { containerAction, pruneContainers } from "@/api/gen/sdk.gen";
import type { ContainerSummary } from "@/api/gen/types.gen";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { columns, DataTable, type Column } from "@/components/DataTable";
import { Page } from "@/components/Page";
import { StateBadge } from "@/components/StateBadge";
import { Button } from "@/components/ui/button";
import { bulkMessage, runBulk } from "@/lib/bulk";
import { formatAgo, formatBytes, shortId } from "@/lib/format";
import { portLinks } from "@/lib/ports";
import { ACTIONS, RemoveDialog } from "./actions";

const col = columns<ContainerSummary>();

function containerColumns(env: string): Column<ContainerSummary>[] {
  return [
    col.accessor("name", {
      header: "Name",
      enableHiding: false,
      cell: (c) => (
        <Link
          to="/envs/$env/containers/$id"
          params={{ env, id: c.row.original.id }}
          className="font-medium hover:underline"
        >
          {c.getValue()}
        </Link>
      ),
    }),
    col.accessor("image", {
      header: "Image",
      // An image with no tag left is listed by its digest.
      cell: (c) => (
        <span className="block max-w-64 truncate">
          {c.getValue().startsWith("sha256:") ? shortId(c.getValue()) : c.getValue()}
        </span>
      ),
    }),
    col.accessor("state", { header: "State", cell: (c) => <StateBadge state={c.getValue()} /> }),
    col.accessor("status", { header: "Status" }),
    col.accessor((c) => c.ports.map((p) => `${p.public_port ?? ""} ${p.private_port}`).join(" "), {
      id: "ports",
      header: "Ports",
      enableSorting: false,
      cell: (c) => <Ports container={c.row.original} />,
    }),
    col.accessor("created", { header: "Created", cell: (c) => formatAgo(c.getValue()) }),
    col.accessor((c) => c.compose_project ?? "", { id: "compose", header: "Compose project" }),
  ];
}

function Ports({ container }: { container: ContainerSummary }) {
  const links = portLinks(container.ports, window.location.hostname);
  return (
    <span className="flex flex-wrap gap-x-2">
      {links.map((l) =>
        l.href ? (
          <a key={l.label} href={l.href} target="_blank" rel="noreferrer" className="text-primary hover:underline">
            {l.label}
          </a>
        ) : (
          <span key={l.label} className="text-muted-foreground">
            {l.label}
          </span>
        ),
      )}
    </span>
  );
}

export function ContainersPage() {
  const { env } = useParams({ from: "/authed/shell/envs/$env/containers" });
  const qc = useQueryClient();
  const list = useQuery(containersQuery(env));
  const [cols] = useState(() => containerColumns(env));
  const [removing, setRemoving] = useState<ContainerSummary[] | null>(null);
  const [pruning, setPruning] = useState(false);
  const refresh = () => qc.invalidateQueries({ queryKey: keys.containers(env) });

  const bulk = useMutation({
    mutationFn: async ({ items, action }: { items: ContainerSummary[]; action: (typeof ACTIONS)[number] }) => {
      const r = await runBulk(items, (c) => c.name, (c) =>
        containerAction({ path: { env, id: c.id, action: action.action } }),
      );
      const msg = bulkMessage(action.done, "container", r);
      if (r.failed.length) toast.error(msg);
      else toast.success(msg);
    },
    onSettled: refresh,
  });

  return (
    <Page title="Containers" actions={<Button variant="outline" onClick={() => setPruning(true)}>Prune</Button>}>
      <DataTable
        label="Containers"
        data={list.data}
        error={list.error}
        columns={cols}
        getRowId={(c) => c.id}
        empty="No containers in this environment."
        initialSorting={[{ id: "name", desc: false }]}
        initialHidden={["compose"]}
        selectable
        actions={(selected, clear) => (
          <>
            {ACTIONS.map((a) => (
              <Button
                key={a.action}
                variant="outline"
                size="sm"
                disabled={bulk.isPending}
                onClick={() => bulk.mutate({ items: selected, action: a }, { onSuccess: clear })}
              >
                {a.label}
              </Button>
            ))}
            <Button variant="destructive" size="sm" onClick={() => setRemoving(selected)}>
              Remove
            </Button>
          </>
        )}
      />
      {removing && <RemoveDialog env={env} containers={removing} onDone={() => setRemoving(null)} />}
      <ConfirmDialog
        open={pruning}
        onOpenChange={setPruning}
        title="Remove all stopped containers?"
        confirmLabel="Prune"
        onConfirm={async () => {
          const { data } = await pruneContainers({ path: { env } });
          toast.success(`Removed ${data.deleted.length} containers, reclaimed ${formatBytes(data.space_reclaimed)}`);
          await refresh();
        }}
      />
    </Page>
  );
}
