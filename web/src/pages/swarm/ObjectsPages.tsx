import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import type { ConfigSummary, SecretSummary, SwarmNetwork } from "@/api/gen/types.gen";
import { swarmConfigsQuery, swarmNetworksQuery, swarmSecretsQuery } from "@/api/queries";
import { columns, DataTable, type Column } from "@/components/DataTable";
import { Page } from "@/components/Page";
import { formatAgo } from "@/lib/format";
import { useSwarmEvents } from "@/live/swarmEvents";
import { useEnv } from "./shared";

type Named = SecretSummary | ConfigSummary;

function namedColumns<T extends Named>(): Column<T>[] {
  const col = columns<T>();
  return [
    col.accessor((x) => x.name, { id: "name", header: "Name", cell: (c) => <span className="font-medium">{c.getValue()}</span> }),
    col.accessor((x) => x.stack ?? "", { id: "stack", header: "Stack" }),
    col.accessor((x) => x.used_by.join(", "), { id: "used_by", header: "Used by" }),
    col.accessor((x) => x.updated_at, { id: "updated", header: "Updated", cell: (c) => formatAgo(c.getValue()) }),
  ];
}

/** Swarm never returns secret values; only who uses each secret is shown. */
export function SecretsPage() {
  const env = useEnv();
  useSwarmEvents(env);
  const q = useQuery(swarmSecretsQuery(env));
  const [cols] = useState(() => namedColumns<SecretSummary>());
  return (
    <Page title="Secrets">
      <DataTable label="Secrets" data={q.data} error={q.error} columns={cols} getRowId={(x) => x.id} empty="No secrets." initialSorting={[{ id: "name", desc: false }]} />
    </Page>
  );
}

export function ConfigsPage() {
  const env = useEnv();
  useSwarmEvents(env);
  const q = useQuery(swarmConfigsQuery(env));
  const [cols] = useState(() => namedColumns<ConfigSummary>());
  return (
    <Page title="Configs">
      <DataTable label="Configs" data={q.data} error={q.error} columns={cols} getRowId={(x) => x.id} empty="No configs." initialSorting={[{ id: "name", desc: false }]} />
    </Page>
  );
}

const ncol = columns<SwarmNetwork>();
const NETWORK_COLUMNS: Column<SwarmNetwork>[] = [
  ncol.accessor("name", { header: "Name", cell: (c) => <span className="font-medium">{c.getValue()}</span> }),
  ncol.accessor((n) => n.stack ?? "", { id: "stack", header: "Stack" }),
  ncol.accessor("driver", { header: "Driver" }),
  ncol.accessor((n) => n.subnets.join(", "), { id: "subnets", header: "Subnets" }),
  ncol.accessor((n) => [n.attachable && "attachable", n.internal && "internal", n.encrypted && "encrypted"].filter(Boolean).join(", "), { id: "flags", header: "Options" }),
  ncol.accessor((n) => n.used_by.join(", "), { id: "used_by", header: "Used by" }),
];

export function NetworksPage() {
  const env = useEnv();
  useSwarmEvents(env);
  const q = useQuery(swarmNetworksQuery(env));
  return (
    <Page title="Networks">
      <DataTable label="Networks" data={q.data} error={q.error} columns={NETWORK_COLUMNS} getRowId={(x) => x.id} empty="No swarm networks." initialSorting={[{ id: "name", desc: false }]} />
    </Page>
  );
}
