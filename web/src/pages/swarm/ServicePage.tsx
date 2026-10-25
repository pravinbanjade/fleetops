import { useQuery } from "@tanstack/react-query";
import { Link, useNavigate, useParams } from "@tanstack/react-router";
import { FileText, ListChecks, ScrollText } from "lucide-react";
import { lazy, Suspense, useState } from "react";
import { useEnvAccess } from "@/api/access";
import { swarmServiceQuery } from "@/api/queries";
import { Button } from "@/components/ui/button";
import { DataTable } from "@/components/DataTable";
import { Page } from "@/components/Page";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { errorMessage } from "@/lib/errors";
import { useSwarmEvents } from "@/live/swarmEvents";
import { useSwarmTasks } from "@/live/swarmTasks";
import { ServiceActions } from "./ServiceActions";
import { ServiceDialog } from "./ServiceDialog";
import { FeedPaused, taskColumns } from "./TasksPage";

const LogsTab = lazy(() => import("@/pages/containers/logs/LogsTab").then((m) => ({ default: m.LogsTab })));

/** One service: its tasks (live), its logs across tasks, and its spec. */
export function ServicePage() {
  const { env = "", id = "" } = useParams({ strict: false }) as { env?: string; id?: string };
  useSwarmEvents(env);
  const svc = useQuery(swarmServiceQuery(env, id));
  const { tasks, paused } = useSwarmTasks(env);
  const [cols] = useState(() => taskColumns(env));
  const [tab, setTab] = useState("tasks");
  const [editing, setEditing] = useState(false);
  const { can } = useEnvAccess(env);
  const navigate = useNavigate();
  const s = svc.data;
  if (svc.error) return <Page title={id}>{<p role="alert" className="text-sm text-destructive">{errorMessage(svc.error)}</p>}</Page>;
  return (
    <Page
      title={s?.name ?? id}
      meta={s && `${s.mode} · ${s.tasks_running}/${s.tasks_desired} running · ${s.image}${s.stack ? ` · stack ${s.stack}` : ""}`}
      actions={
        s && (
          <span className="flex items-center gap-2">
            {can("swarm-services.operate") && (
              <Button variant="outline" onClick={() => setEditing(true)}>
                Edit
              </Button>
            )}
            <ServiceActions env={env} service={s} onRemoved={() => void navigate({ to: "/envs/$env/swarm/services", params: { env } })} />
          </span>
        )
      }
    >
      {editing && s && <ServiceDialog env={env} target={{ mode: "edit", service: s }} onClose={() => setEditing(false)} />}
      {s?.stack && s.labels["io.fleetops.stack.spec-hash"] && (
        <p className="rounded-md border px-3 py-2 text-sm">
          Managed by stack{" "}
          <Link to="/envs/$env/stacks/$stack" params={{ env, stack: s.stack }} className="font-ident font-medium hover:underline">
            {s.stack}
          </Link>
          ; changes here are overwritten on the next deploy.
        </p>
      )}
      <Tabs value={tab} onValueChange={setTab}>
        <TabsList>
          <TabsTrigger value="tasks">
            <ListChecks /> Tasks
          </TabsTrigger>
          <TabsTrigger value="logs">
            <ScrollText /> Logs
          </TabsTrigger>
          <TabsTrigger value="spec">
            <FileText /> Spec
          </TabsTrigger>
        </TabsList>
        <TabsContent value="tasks">
          <FeedPaused reason={paused} />
          <DataTable
            label="Tasks"
            data={tasks?.filter((t) => t.service_id === id)}
            columns={cols}
            getRowId={(t) => t.id}
            empty="No tasks."
            initialSorting={[{ id: "updated_at", desc: true }]}
          />
        </TabsContent>
        <TabsContent value="logs">
          {tab === "logs" && (
            <Suspense fallback={null}>
              <LogsTab env={env} id={`service/${id}`} name={s?.name ?? id} running />
            </Suspense>
          )}
        </TabsContent>
        <TabsContent value="spec">
          <pre className="overflow-auto rounded-md bg-muted p-3 font-mono text-xs">{s ? JSON.stringify(s.spec, null, 2) : ""}</pre>
        </TabsContent>
      </Tabs>
    </Page>
  );
}
