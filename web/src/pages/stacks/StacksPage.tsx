import { useQuery } from "@tanstack/react-query";
import { Link, getRouteApi } from "@tanstack/react-router";
import { useMemo, useState } from "react";
import type { StackSummary } from "@/api/gen/types.gen";
import { useEnvAccess } from "@/api/access";
import { composeProjectsQuery, stacksQuery } from "@/api/queries";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { DataTable, columns } from "@/components/DataTable";
import { Page } from "@/components/Page";
import { StateBadge } from "@/components/StateBadge";
import { formatAgo } from "@/lib/format";
import { errorMessage } from "@/lib/errors";
import { ProjectCard } from "@/pages/compose/ProjectCard";
import { NewStackDialog } from "./NewStackDialog";

const route = getRouteApi("/authed/shell/envs/$env/stacks");

function stackColumns(env: string) {
  const col = columns<StackSummary>();
  return [
    col.accessor("name", {
      header: "Name",
      enableHiding: false,
      cell: (c) => (
        <Link
          to="/envs/$env/stacks/$stack"
          params={{ env, stack: c.getValue() }}
          className="font-ident font-medium hover:underline"
        >
          {c.getValue()}
        </Link>
      ),
    }),
    col.accessor("last_status", {
      header: "Status",
      cell: (c) => (
        <span className="flex items-center gap-2">
          {c.getValue() ? <StateBadge state={c.getValue()!} /> : <span className="text-muted-foreground">never deployed</span>}
          {c.row.original.draft_differs && c.row.original.deployed_revision != null && (
            <Badge variant="outline">Update pending</Badge>
          )}
        </span>
      ),
    }),
    col.accessor("deployed_revision", {
      header: "Revision",
      cell: (c) => (c.getValue() != null ? `#${c.getValue()}` : ""),
    }),
    col.accessor("source", { header: "Source" }),
    col.accessor("updated_at", { header: "Updated", cell: (c) => formatAgo(c.getValue()) }),
  ];
}

/** FleetOps stacks, then the Compose projects FleetOps does not manage. */
export function StacksPage() {
  const { env } = route.useParams();
  const stacks = useQuery(stacksQuery(env));
  const projects = useQuery(composeProjectsQuery(env));
  const cols = useMemo(() => stackColumns(env), [env]);
  const { can } = useEnvAccess(env);
  const [creating, setCreating] = useState(false);
  const unmanaged = (projects.data ?? []).filter((p) => !p.managed);
  return (
    <Page
      title="Stacks"
      actions={
        can("stacks.create") && (
          <Button size="sm" onClick={() => setCreating(true)}>
            New stack
          </Button>
        )
      }
    >
      {creating && <NewStackDialog env={env} onClose={() => setCreating(false)} />}
      <DataTable
        label="Stacks"
        data={stacks.data}
        columns={cols}
        getRowId={(r) => r.name}
        error={stacks.error}
        empty="No stacks in this environment yet."
      />
      {projects.error ? (
        <p role="alert" className="text-sm text-destructive">
          {errorMessage(projects.error)}
        </p>
      ) : (
        unmanaged.length > 0 && (
          <section className="grid gap-4" aria-labelledby="other-compose">
            <div className="grid gap-1">
              <h2 id="other-compose" className="text-base font-semibold">
                Other Compose projects
              </h2>
              <p className="text-sm text-muted-foreground">
                Deployed outside FleetOps, read from the labels Docker Compose puts on containers. Create a stack with
                the same name to manage one here.
              </p>
            </div>
            {unmanaged.map((p) => (
              <ProjectCard key={p.name} env={env} project={p} />
            ))}
          </section>
        )
      )}
    </Page>
  );
}
