import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { toast } from "sonner";
import { envStatusQuery } from "@/api/queries";
import { finishSwarmRotation, listSwarmAgents, retrustSwarm } from "@/api/gen/sdk.gen";
import type { SwarmAgentView } from "@/api/gen/types.gen";
import { useSession } from "@/auth/session";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { columns, DataTable, type Column } from "@/components/DataTable";
import { Page } from "@/components/Page";
import { Button } from "@/components/ui/button";
import { formatAgo } from "@/lib/format";

const col = columns<SwarmAgentView>();

const COLUMNS: Column<SwarmAgentView>[] = [
  col.accessor((n) => n.hostname ?? n.node_id, {
    id: "node",
    header: "Node",
    cell: (c) => (
      <span className="grid">
        <span className="font-medium">{c.getValue()}</span>
        <code className="text-xs text-muted-foreground">{c.row.original.node_id}</code>
      </span>
    ),
  }),
  col.accessor((n) => n.role ?? "", { id: "role", header: "Role" }),
  col.accessor((n) => n.state ?? "", { id: "state", header: "State" }),
  col.accessor("connected", {
    header: "Agent",
    cell: (c) => (c.getValue() ? <span>connected</span> : <span className="text-muted-foreground">no agent</span>),
  }),
  col.accessor((n) => n.agent_version ?? "", {
    id: "version",
    header: "Version",
    cell: (c) => <span className="font-mono text-xs">{c.getValue()}</span>,
  }),
  col.accessor((n) => n.last_seen_at ?? 0, {
    id: "seen",
    header: "Last seen",
    cell: (c) => (c.row.original.connected || !c.row.original.last_seen_at ? "" : formatAgo(c.row.original.last_seen_at)),
  }),
  col.accessor((n) => n.last_error ?? "", {
    id: "error",
    header: "Error",
    cell: (c) => <span className="text-xs text-destructive">{c.getValue()}</span>,
  }),
];

/** `/envs/$env` for a Swarm: every node and whether its agent is connected. */
export function SwarmHome({ env }: { env: string }) {
  const qc = useQueryClient();
  const { user } = useSession();
  const status = useQuery(envStatusQuery(env, true));
  const agentsKey = ["env", env, "swarm", "agents"] as const;
  const agents = useQuery({
    queryKey: agentsKey,
    queryFn: async () => (await listSwarmAgents({ path: { env }, throwOnError: true })).data,
  });
  // Agents come and go with the environment's status.
  useEffect(() => {
    void qc.invalidateQueries({ queryKey: agentsKey });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [status.dataUpdatedAt]);
  const [confirm, setConfirm] = useState<"finish" | "retrust" | null>(null);
  const rows = agents.data ?? [];
  const covered = rows.filter((n) => n.connected).length;
  const down = status.data?.state === "down" ? status.data.reason : null;
  return (
    <Page
      title="Nodes"
      meta={agents.data && `${covered} of ${rows.length} nodes have an agent`}
      actions={
        user.role === "admin" && (
          <span className="flex gap-2">
            <Button variant="outline" onClick={() => setConfirm("finish")}>
              Finish key rotation
            </Button>
            <Button variant="outline" onClick={() => setConfirm("retrust")}>
              Re-trust swarm
            </Button>
          </span>
        )
      }
    >
      {down && (
        <p role="status" className="mb-3 rounded-md border border-destructive/40 bg-destructive/5 p-3 text-sm">
          {down}
        </p>
      )}
      <DataTable label="Nodes" data={agents.data} error={agents.error} columns={COLUMNS} getRowId={(n) => n.node_id} empty="No nodes yet." />
      {confirm === "finish" && (
        <ConfirmDialog
          open
          onOpenChange={(open) => !open && setConfirm(null)}
          title="Finish key rotation?"
          description="The key from before the last rotation stops working now, and agents still using it disconnect. Do this once every node runs with the new secret."
          confirmLabel="Finish rotation"
          onConfirm={async () => {
            await finishSwarmRotation({ path: { env }, throwOnError: true });
            toast.success("Rotation finished");
            await qc.invalidateQueries({ queryKey: agentsKey });
          }}
        />
      )}
      {confirm === "retrust" && (
        <ConfirmDialog
          open
          onOpenChange={(open) => !open && setConfirm(null)}
          title="Re-trust this swarm?"
          description="Use this after rotating the swarm's root CA (docker swarm ca --rotate). Every agent disconnects; the first manager agent to reconnect sets the swarm and root CA FleetOps trusts."
          confirmLabel="Re-trust"
          onConfirm={async () => {
            await retrustSwarm({ path: { env }, throwOnError: true });
            toast.success("Swarm unpinned; agents are reconnecting");
            await qc.invalidateQueries({ queryKey: agentsKey });
          }}
        />
      )}
    </Page>
  );
}
