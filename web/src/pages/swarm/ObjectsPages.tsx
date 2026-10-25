import { useQuery } from "@tanstack/react-query";
import { useMemo, useState } from "react";
import type { ConfigSummary, SecretSummary, SwarmNetwork } from "@/api/gen/types.gen";
import { swarmConfigsQuery, swarmNetworksQuery, swarmSecretsQuery } from "@/api/queries";
import { columns, DataTable, type Column } from "@/components/DataTable";
import { Page } from "@/components/Page";
import { formatAgo } from "@/lib/format";
import { useEnvAccess } from "@/api/access";
import { Button } from "@/components/ui/button";
import { useSwarmEvents } from "@/live/swarmEvents";
import { CreateObjectDialog, CreateSwarmNetworkDialog, ObjectActions, permOf } from "./ObjectDialogs";
import { useEnv } from "./shared";

type Named = SecretSummary | ConfigSummary;

function namedColumns<T extends Named>(env: string, kind: "secret" | "config"): Column<T>[] {
  const col = columns<T>();
  return [
    col.accessor((x) => x.name, { id: "name", header: "Name", cell: (c) => <span className="font-medium">{c.getValue()}</span> }),
    col.accessor((x) => x.stack ?? "", { id: "stack", header: "Stack" }),
    col.accessor((x) => x.used_by.join(", "), { id: "used_by", header: "Used by" }),
    col.accessor((x) => x.updated_at, { id: "updated", header: "Updated", cell: (c) => formatAgo(c.getValue()) }),
    col.display({ id: "actions", header: "", cell: (c) => <ObjectActions env={env} kind={kind} id={c.row.original.id} name={c.row.original.name} stack={c.row.original.stack ?? null} /> }),
  ];
}

/** Swarm never returns secret values; only who uses each secret is shown. */
export function SecretsPage() {
  const env = useEnv();
  useSwarmEvents(env);
  const q = useQuery(swarmSecretsQuery(env));
  const cols = useMemo(() => namedColumns<SecretSummary>(env, "secret"), [env]);
  const [creating, setCreating] = useState(false);
  const { can } = useEnvAccess(env);
  return (
    <Page title="Secrets" actions={can(permOf("secret", "create"), null) && <Button onClick={() => setCreating(true)}>Create secret</Button>}>
      {creating && <CreateObjectDialog env={env} kind="secret" onClose={() => setCreating(false)} />}
      <DataTable label="Secrets" data={q.data} error={q.error} columns={cols} getRowId={(x) => x.id} empty="No secrets." initialSorting={[{ id: "name", desc: false }]} />
    </Page>
  );
}

export function ConfigsPage() {
  const env = useEnv();
  useSwarmEvents(env);
  const q = useQuery(swarmConfigsQuery(env));
  const cols = useMemo(() => namedColumns<ConfigSummary>(env, "config"), [env]);
  const [creating, setCreating] = useState(false);
  const { can } = useEnvAccess(env);
  return (
    <Page title="Configs" actions={can(permOf("config", "create"), null) && <Button onClick={() => setCreating(true)}>Create config</Button>}>
      {creating && <CreateObjectDialog env={env} kind="config" onClose={() => setCreating(false)} />}
      <DataTable label="Configs" data={q.data} error={q.error} columns={cols} getRowId={(x) => x.id} empty="No configs." initialSorting={[{ id: "name", desc: false }]} />
    </Page>
  );
}

const ncol = columns<SwarmNetwork>();
const networkColumns = (env: string): Column<SwarmNetwork>[] => [
  ncol.accessor("name", { header: "Name", cell: (c) => <span className="font-medium">{c.getValue()}</span> }),
  ncol.accessor((n) => n.stack ?? "", { id: "stack", header: "Stack" }),
  ncol.accessor("driver", { header: "Driver" }),
  ncol.accessor((n) => n.subnets.join(", "), { id: "subnets", header: "Subnets" }),
  ncol.accessor((n) => [n.attachable && "attachable", n.internal && "internal", n.encrypted && "encrypted"].filter(Boolean).join(", "), { id: "flags", header: "Options" }),
  ncol.accessor((n) => n.used_by.join(", "), { id: "used_by", header: "Used by" }),
  ncol.display({ id: "actions", header: "", cell: (c) => <ObjectActions env={env} kind="network" id={c.row.original.id} name={c.row.original.name} stack={c.row.original.stack ?? null} /> }),
];

export function NetworksPage() {
  const env = useEnv();
  useSwarmEvents(env);
  const q = useQuery(swarmNetworksQuery(env));
  const cols = useMemo(() => networkColumns(env), [env]);
  const [creating, setCreating] = useState(false);
  const { can } = useEnvAccess(env);
  return (
    <Page title="Networks" actions={can(permOf("network", "create"), null) && <Button onClick={() => setCreating(true)}>Create network</Button>}>
      {creating && <CreateSwarmNetworkDialog env={env} onClose={() => setCreating(false)} />}
      <DataTable label="Networks" data={q.data} error={q.error} columns={cols} getRowId={(x) => x.id} empty="No swarm networks." initialSorting={[{ id: "name", desc: false }]} />
    </Page>
  );
}
