import { useNow, useNowRow } from "@/components/shell/now";
import { ObjectNow } from "@/components/shell/ObjectNow";
import { Tick } from "@/components/Tick";
import { useSwarmTasks } from "@/live/swarmTasks";
import { taskName } from "./shared";
import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useMemo, useState } from "react";
import type { ServiceSummary } from "@/api/gen/types.gen";
import { swarmServicesQuery } from "@/api/queries";
import { columns, DataTable, type Column } from "@/components/DataTable";
import { Page } from "@/components/Page";
import { useEnvAccess } from "@/api/access";
import { Button } from "@/components/ui/button";
import { useSwarmEvents } from "@/live/swarmEvents";
import { ServiceActions } from "./ServiceActions";
import { ServiceDialog } from "./ServiceDialog";
import { useEnv } from "./shared";

const col = columns<ServiceSummary>();

const serviceColumns = (env: string): Column<ServiceSummary>[] => [
  col.accessor("name", {
    header: "Name",
    cell: (c) => (
      <Link to="/envs/$env/swarm/services/$id" params={{ env, id: c.row.original.id }} className="font-medium hover:underline">
        {c.getValue()}
      </Link>
    ),
  }),
  col.accessor((s) => s.stack ?? "", { id: "stack", header: "Stack" }),
  col.accessor("mode", { header: "Mode" }),
  col.accessor((s) => `${s.tasks_running}/${s.tasks_desired}`, {
    id: "replicas",
    header: "Running",
    cell: (c) => (
      <span className={c.row.original.tasks_running < c.row.original.tasks_desired ? "text-state-fail" : undefined}>{c.getValue()}</span>
    ),
  }),
  col.accessor("image", { header: "Image", cell: (c) => <span className="font-mono text-xs">{c.getValue()}</span> }),
  col.accessor((s) => s.ports.join(", "), { id: "ports", header: "Ports" }),
  col.accessor((s) => s.update_state ?? "", { id: "update", header: "Update" }),
  col.display({ id: "actions", header: "", cell: (c) => <ServiceActions env={env} service={c.row.original} /> }),
];

export function ServicesPage() {
  const env = useEnv();
  useSwarmEvents(env);
  const services = useQuery(swarmServicesQuery(env));
  const now = useNow();
  const active = useNowRow(services.data, (s: ServiceSummary) => s.id);
  const cols = useMemo(() => serviceColumns(env), [env]);
  const [creating, setCreating] = useState(false);
  const { can } = useEnvAccess(env);
  return (
    <Page title="Services" actions={can("swarm-services.create", null) && <Button onClick={() => setCreating(true)}>Create service</Button>}>
      {creating && <ServiceDialog env={env} target={{ mode: "create" }} onClose={() => setCreating(false)} />}
      {active && <ServiceNow key={active.id} env={env} service={active} />}
      <DataTable label="Services" data={services.data} error={services.error} columns={cols} onActivate={(r) => now.open(((s: ServiceSummary) => s.id)(r))} activeId={active ? ((s: ServiceSummary) => s.id)(active) : undefined} getRowId={(s) => s.id} empty="No services." initialSorting={[{ id: "name", desc: false }]} />
    </Page>
  );
}

/** A service in the panel: its tasks running against those wanted, live. */
function ServiceNow({ env, service: s }: { env: string; service: ServiceSummary }) {
  const { tasks: all } = useSwarmTasks(env);
  const tasks = (all ?? []).filter((t) => t.service_id === s.id);
  const running = all ? tasks.filter((t) => t.state === "running").length : s.tasks_running;
  return (
    <ObjectNow
      title={s.name}
      subtitle={<span className="font-ident">{s.image}</span>}
      facts={[
        { label: "Running", value: <Tick value={running} text={`${running} / ${s.tasks_desired}`} className={running < s.tasks_desired ? "text-fall" : undefined} /> },
        { label: "Mode", value: s.mode },
        { label: "Update", value: s.update_state ?? "—" },
      ]}
      list={{
        title: "Tasks",
        empty: all ? "No tasks." : "Loading tasks…",
        rows: tasks
          .slice()
          .sort((a, b) => b.updated_at - a.updated_at)
          .slice(0, 30)
          .map((t) => ({ key: t.id, label: taskName(t), aside: t.error ? t.error : t.state, failing: !!t.error || t.state === "failed" || t.state === "rejected" })),
      }}
      open={{ to: "/envs/$env/swarm/services/$id", params: { env, id: s.id } }}
    />
  );
}
