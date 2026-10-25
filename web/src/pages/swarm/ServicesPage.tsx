import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useState } from "react";
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
  const [cols] = useState(() => serviceColumns(env));
  const [creating, setCreating] = useState(false);
  const { can } = useEnvAccess(env);
  return (
    <Page title="Services" actions={can("swarm-services.create") && <Button onClick={() => setCreating(true)}>Create service</Button>}>
      {creating && <ServiceDialog env={env} target={{ mode: "create" }} onClose={() => setCreating(false)} />}
      <DataTable label="Services" data={services.data} error={services.error} columns={cols} getRowId={(s) => s.id} empty="No services." initialSorting={[{ id: "name", desc: false }]} />
    </Page>
  );
}
