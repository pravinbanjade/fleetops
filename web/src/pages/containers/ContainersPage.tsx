import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { getRouteApi, Link } from "@tanstack/react-router";
import { ArrowUpRight, Plus, Trash2 } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import { containersQuery, keys } from "@/api/queries";
import { containerAction, pruneContainers } from "@/api/gen/sdk.gen";
import type { ContainerSummary } from "@/api/gen/types.gen";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { columns, DataTable, type Column } from "@/components/DataTable";
import { Page } from "@/components/Page";
import { ContainerState, stateTone } from "@/components/StateBadge";
import { needsAttention, shownState } from "@/lib/health";
import { Button } from "@/components/ui/button";
import { bulkMessage, runBulk } from "@/lib/bulk";
import { formatAgo, formatBytes, shortId } from "@/lib/format";
import { portLinks } from "@/lib/ports";
import { Usage } from "@/components/Usage";
import { ACTIONS, RemoveDialog } from "./actions";

const route = getRouteApi("/authed/shell/envs/$env/containers");
const col = columns<ContainerSummary>();

const RANK = { fail: 0, wait: 1, run: 2, idle: 3 } as const;
const BAYS = [
  { id: "attention", label: "Needs attention" },
  { id: "transitional", label: "Changing" },
  { id: "running", label: "Running" },
  { id: "stopped", label: "Stopped" },
];

/** Where a container sits in the rack: the ones that need someone first. */
function urgency(c: ContainerSummary): number {
  return needsAttention(c) ? 0 : RANK[stateTone(shownState(c))];
}

function containerColumns(env: string): Column<ContainerSummary>[] {
  return [
    // The holder leads the strip, and the rack sorts by urgency: failed,
    // then transitional, then running, then stopped.
    col.accessor("state", {
      header: "State",
      cell: (c) => <ContainerState container={c.row.original} />,
      sortFn: (a, b) => urgency(a.original) - urgency(b.original),
    }),
    col.accessor("name", {
      header: "Name",
      enableHiding: false,
      cell: (c) => (
        <Link
          to="/envs/$env/containers/$id"
          params={{ env, id: c.row.original.id }}
          className="font-semibold hover:underline"
        >
          {c.getValue()}
        </Link>
      ),
    }),
    col.accessor("image", {
      header: "Image",
      // An image with no tag left is listed by its digest.
      cell: (c) => (
        <span className="block max-w-64 truncate text-muted-foreground">
          {c.getValue().startsWith("sha256:") ? shortId(c.getValue()) : c.getValue()}
        </span>
      ),
    }),
    col.accessor("status", {
      header: "Status",
      cell: (c) => <span className="text-muted-foreground">{c.getValue()}</span>,
    }),
    col.accessor((c) => c.ports.map((p) => `${p.public_port ?? ""} ${p.private_port}`).join(" "), {
      id: "ports",
      header: "Ports",
      enableSorting: false,
      cell: (c) => <Ports container={c.row.original} />,
    }),
    col.display({ id: "cpu", header: "CPU", cell: (c) => <Usage env={env} container={c.row.original} kind="cpu" /> }),
    col.display({
      id: "memory",
      header: "Memory",
      cell: (c) => <Usage env={env} container={c.row.original} kind="memory" />,
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
          <a
            key={l.label}
            href={l.href}
            target="_blank"
            rel="noreferrer"
            className="inline-flex items-center gap-0.5 font-mono text-xs underline decoration-border hover:decoration-primary"
          >
            {l.label}
            <ArrowUpRight aria-hidden className="size-3 text-muted-foreground" />
          </a>
        ) : (
          <span key={l.label} className="font-mono text-xs text-muted-foreground">
            {l.label}
          </span>
        ),
      )}
    </span>
  );
}

export function ContainersPage() {
  const { env } = route.useParams();
  const { q } = route.useSearch();
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
    <Page
      title="Containers"
      actions={
        <>
          <Button variant="outline" onClick={() => setPruning(true)}>
            <Trash2 />
            Prune
          </Button>
          <Button asChild>
            <Link to="/envs/$env/containers/new" params={{ env }}>
              <Plus />
              Create container
            </Link>
          </Button>
        </>
      }
    >
      <DataTable
        label="Containers"
        data={list.data}
        error={list.error}
        columns={cols}
        getRowId={(c) => c.id}
        empty="No containers in this environment."
        initialSorting={[{ id: "state", desc: false }, { id: "name", desc: false }]}
        initialHidden={["compose"]}
        initialFilter={q}
        bays={{ column: "state", of: (c) => BAYS[urgency(c)] }}
        phoneColumns={["state", "name", "status"]}
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
                <a.icon />
                {a.label}
              </Button>
            ))}
            <Button variant="destructive" size="sm" onClick={() => setRemoving(selected)}>
              <Trash2 />
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
