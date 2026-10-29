import { useNow, useNowRow } from "@/components/shell/now";
import { ObjectNow } from "@/components/shell/ObjectNow";
import { useQuery } from "@tanstack/react-query";
import { Link, getRouteApi } from "@tanstack/react-router";
import { GitBranch, Plus } from "lucide-react";
import { useMemo, useState } from "react";
import type { ServiceSummary, StackSummary } from "@/api/gen/types.gen";
import { useEnvAccess, useEnvFamily } from "@/api/access";
import { composeProjectsQuery, containersQuery, stacksQuery, swarmServicesQuery } from "@/api/queries";
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
          {c.row.original.source !== "git" && c.row.original.draft_differs && c.row.original.deployed_revision != null && (
            <Badge variant="outline">Update pending</Badge>
          )}
          {c.row.original.update_available && <Badge variant="outline">Update available</Badge>}
          {c.row.original.paused && <Badge variant="outline">Paused</Badge>}
          {c.row.original.sync_error && (
            <Badge variant="destructive" title={c.row.original.sync_error}>
              Sync error
            </Badge>
          )}
        </span>
      ),
    }),
    col.accessor("deployed_revision", {
      header: "Revision",
      cell: (c) => (c.getValue() != null ? `#${c.getValue()}` : ""),
    }),
    col.accessor("source", {
      header: "Source",
      cell: (c) =>
        c.getValue() === "git" ? (
          <span className="inline-flex items-center gap-1">
            <GitBranch className="size-3.5" aria-hidden />
            git
          </span>
        ) : (
          c.getValue()
        ),
    }),
    col.accessor("updated_at", { header: "Updated", cell: (c) => formatAgo(c.getValue()) }),
  ];
}

/**
 * Stacks deployed to a swarm outside FleetOps (`docker stack deploy`), from
 * the stack label on their services. Creating a FleetOps stack of the same
 * name takes them over: its first deploy updates them in place.
 */
function OtherSwarmStacks({ env, managed }: { env: string; managed: Set<string> }) {
  const services = useQuery(swarmServicesQuery(env));
  const byStack = new Map<string, ServiceSummary[]>();
  for (const s of services.data ?? []) {
    if (s.stack && !managed.has(s.stack)) byStack.set(s.stack, [...(byStack.get(s.stack) ?? []), s]);
  }
  if (services.error)
    return (
      <p role="alert" className="text-sm text-destructive">
        {errorMessage(services.error)}
      </p>
    );
  if (byStack.size === 0) return null;
  return (
    <section className="grid gap-4" aria-labelledby="other-stacks">
      <div className="grid gap-1">
        <h2 id="other-stacks" className="text-base font-semibold">
          Other stacks
        </h2>
        <p className="text-sm text-muted-foreground">
          Deployed outside FleetOps, read from the stack label on their services. Create a stack with the same name to
          manage one here: its first deploy updates these services in place.
        </p>
      </div>
      <ul className="grid gap-3">
        {[...byStack.entries()]
          .sort(([a], [b]) => a.localeCompare(b))
          .map(([stack, list]) => (
            <li key={stack} className="grid gap-1 rounded-md bg-raised px-3 py-2">
              <span className="font-ident font-medium">{stack}</span>
              <ul className="flex flex-wrap gap-x-4 gap-y-1 text-sm">
                {list.map((s) => (
                  <li key={s.id}>
                    <Link
                      to="/envs/$env/swarm/services/$id"
                      params={{ env, id: s.id }}
                      className="font-ident hover:underline"
                    >
                      {s.name}
                    </Link>
                  </li>
                ))}
              </ul>
            </li>
          ))}
      </ul>
    </section>
  );
}

/** FleetOps stacks, then the Compose projects (or Swarm stacks) FleetOps does not manage. */
export function StacksPage() {
  const { env } = route.useParams();
  const swarm = useEnvFamily(env) === "swarm";
  const stacks = useQuery(stacksQuery(env));
  const now = useNow();
  const active = useNowRow(stacks.data, (r: StackSummary) => r.name);
  const projects = useQuery({ ...composeProjectsQuery(env), enabled: !swarm });
  const cols = useMemo(() => stackColumns(env), [env]);
  const { can } = useEnvAccess(env);
  const [creating, setCreating] = useState(false);
  const unmanaged = (projects.data ?? []).filter((p) => !p.managed);
  const managed = new Set((stacks.data ?? []).map((s) => s.name));
  return (
    <Page
      title="Stacks"
      actions={
        can("stacks.create") && (
          <Button size="sm" onClick={() => setCreating(true)}>
            <Plus aria-hidden />
            New stack
          </Button>
        )
      }
    >
      {creating && <NewStackDialog env={env} onClose={() => setCreating(false)} />}
      {active && <StackNow key={active.name} env={env} stack={active} swarm={swarm} />}
      <DataTable
        label="Stacks"
        data={stacks.data}
        columns={cols}
        onActivate={(r) => now.open(((r: StackSummary) => r.name)(r))} activeId={active ? ((r: StackSummary) => r.name)(active) : undefined} getRowId={(r) => r.name}
        error={stacks.error}
        empty="No stacks in this environment yet."
      />
      {swarm ? (
        can("swarm-services.view") && <OtherSwarmStacks env={env} managed={managed} />
      ) : projects.error ? (
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

/** A stack in the panel: its revision and last result, and what it runs now. */
function StackNow({ env, stack, swarm }: { env: string; stack: StackSummary; swarm: boolean }) {
  const containers = useQuery({ ...containersQuery(env), enabled: !swarm });
  const services = useQuery({ ...swarmServicesQuery(env), enabled: swarm });
  const rows = swarm
    ? (services.data ?? [])
        .filter((s) => s.stack === stack.name)
        .map((s) => ({
          key: s.id,
          label: s.name,
          aside: `${s.tasks_running} / ${s.tasks_desired}`,
          failing: s.tasks_running < s.tasks_desired,
          link: { to: "/envs/$env/swarm/services/$id", params: { env, id: s.id } },
        }))
    : (containers.data ?? [])
        .filter((c) => c.compose_project === stack.name)
        .map((c) => ({ key: c.id, label: c.name, aside: c.state, failing: c.state !== "running", link: { to: "/envs/$env/containers/$id", params: { env, id: c.id } } }));
  return (
    <ObjectNow
      title={stack.name}
      subtitle={stack.sync_error ?? stack.last_status ?? undefined}
      facts={[
        { label: "Revision", value: <span className="font-figure">{stack.deployed_revision ?? "—"}</span> },
        { label: "Source", value: stack.source },
        { label: "Updated", value: formatAgo(stack.updated_at) },
      ]}
      list={{ title: swarm ? "Services" : "Containers", empty: "Nothing deployed yet.", rows }}
      open={{ to: "/envs/$env/stacks/$stack", params: { env, stack: stack.name } }}
    />
  );
}
