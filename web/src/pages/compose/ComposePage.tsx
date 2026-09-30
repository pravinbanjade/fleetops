import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useParams } from "@tanstack/react-router";
import { useState } from "react";
import { toast } from "sonner";
import { composeProjectsQuery, containersQuery, keys } from "@/api/queries";
import { composeAction } from "@/api/gen/sdk.gen";
import type { ComposeActionName, ComposeProject } from "@/api/gen/types.gen";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { Page } from "@/components/Page";
import { StateBadge } from "@/components/StateBadge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { errorMessage } from "@/lib/errors";

const DONE: Record<ComposeActionName, string> = {
  start: "Started",
  stop: "Stopped",
  restart: "Restarted",
  down: "Removed",
};

/** Compose projects, from the containers' `com.docker.compose.project` labels. */
export function ComposePage() {
  const { env } = useParams({ from: "/authed/shell/envs/$env/compose" });
  const projects = useQuery(composeProjectsQuery(env));
  return (
    <Page title="Compose projects">
      <p className="text-sm text-muted-foreground">
        Projects are read from the labels Docker Compose puts on containers. Editing and deploying stacks comes in a
        later release.
      </p>
      {projects.error ? (
        <p role="alert" className="text-sm text-destructive">
          {errorMessage(projects.error)}
        </p>
      ) : !projects.data ? (
        <p className="text-sm text-muted-foreground">Loading…</p>
      ) : projects.data.length === 0 ? (
        <p className="text-sm text-muted-foreground">No compose projects in this environment.</p>
      ) : (
        <div className="grid gap-4">
          {projects.data.map((p) => (
            <ProjectCard key={p.name} env={env} project={p} />
          ))}
        </div>
      )}
    </Page>
  );
}

function ProjectCard({ env, project }: { env: string; project: ComposeProject }) {
  const qc = useQueryClient();
  const containers = useQuery(containersQuery(env));
  const [downing, setDowning] = useState(false);
  const members = (containers.data ?? []).filter((c) => c.compose_project === project.name);
  const run = async (action: ComposeActionName) => {
    const { data } = await composeAction({ path: { env, project: project.name, action } });
    await qc.invalidateQueries({ queryKey: keys.containers(env) });
    await qc.invalidateQueries({ queryKey: keys.composeProjects(env) });
    const n = data.affected;
    const msg = `${DONE[action]} ${n} container${n === 1 ? "" : "s"} of ${project.name}`;
    if (data.errors.length) toast.error(`${msg}. ${data.errors.join("; ")}`);
    else toast.success(msg);
  };
  const act = useMutation({ mutationFn: run });
  return (
    <Card aria-label={project.name} role="region">
      <CardHeader className="flex flex-row flex-wrap items-center justify-between gap-2">
        <CardTitle className="flex items-center gap-3">
          {project.name}
          <StateBadge state={project.status} />
          <span className="text-sm font-normal text-muted-foreground">
            {project.running} of {project.containers} running
          </span>
        </CardTitle>
        <div className="flex flex-wrap gap-2">
          {(["start", "stop", "restart"] as const).map((a) => (
            <Button key={a} variant="outline" size="sm" disabled={act.isPending} onClick={() => act.mutate(a)}>
              {a[0].toUpperCase() + a.slice(1)}
            </Button>
          ))}
          <Button variant="destructive" size="sm" onClick={() => setDowning(true)}>
            Down
          </Button>
        </div>
      </CardHeader>
      <CardContent className="grid gap-2 text-sm">
        <div className="text-muted-foreground">Services: {project.services.join(", ")}</div>
        <ul className="grid gap-1" aria-label={`Containers of ${project.name}`}>
          {members.map((c) => (
            <li key={c.id} className="flex items-center gap-2">
              <Link to="/envs/$env/containers/$id" params={{ env, id: c.id }} className="font-medium hover:underline">
                {c.name}
              </Link>
              <StateBadge state={c.state} />
              <span className="text-muted-foreground">{c.status}</span>
            </li>
          ))}
        </ul>
      </CardContent>
      <ConfirmDialog
        open={downing}
        onOpenChange={setDowning}
        title={`Take ${project.name} down?`}
        description="Stops and removes the project's containers. Volumes, networks and images are kept."
        confirmLabel="Down"
        onConfirm={() => run("down")}
      />
    </Card>
  );
}
