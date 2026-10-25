import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { toast } from "sonner";
import { useEnvAccess } from "@/api/access";
import { envStatusQuery } from "@/api/queries";
import { Link } from "@tanstack/react-router";
import { finishSwarmRotation, getSwarmTrust, listSwarmAgents, retrustSwarm, trustSwarm } from "@/api/gen/sdk.gen";
import { ApiError } from "@/api/client";
import type { SwarmAgentView } from "@/api/gen/types.gen";
import { useSession } from "@/auth/session";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { columns, DataTable, type Column } from "@/components/DataTable";
import { Page } from "@/components/Page";
import { Button } from "@/components/ui/button";
import { errorMessage } from "@/lib/errors";
import { formatAgo } from "@/lib/format";
import { NodeActions } from "./NodeActions";

const col = columns<SwarmAgentView>();

/** The node table's columns; a node's name opens its node view. */
const nodeColumns = (env: string): Column<SwarmAgentView>[] => [
  col.accessor((n) => n.hostname ?? n.node_id, {
    id: "node",
    header: "Node",
    cell: (c) => (
      <span className="grid">
        <Link
          to="/envs/$env"
          params={{ env: `${env}@${c.row.original.node_id}` }}
          className="font-medium hover:underline"
        >
          {c.getValue()}
        </Link>
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
  col.display({
    id: "actions",
    header: "",
    cell: (c) => {
      const n = c.row.original;
      return <NodeActions env={env} node={{ id: n.node_id, hostname: n.hostname ?? n.node_id, role: n.role, state: n.state, availability: n.availability }} />;
    },
  }),
];

/** Shown while a manager has proposed the swarm and no admin has trusted it yet. */
export function TrustCard({ env, admin }: { env: string; admin: boolean }) {
  const qc = useQueryClient();
  const { can } = useEnvAccess(env);
  const trust = useQuery({
    queryKey: ["env", env, "swarm", "trust"],
    // The server answers only grants holding the nodes row.
    enabled: can("swarm-nodes.view"),
    queryFn: async () => {
      try {
        return (await getSwarmTrust({ path: { env }, throwOnError: true })).data;
      } catch (e) {
        // Nothing proposed yet.
        if (e instanceof ApiError && e.status === 404) return null;
        throw e;
      }
    },
  });
  const [reject, setReject] = useState(false);
  const t = trust.data;
  if (!t || t.confirmed) return null;
  if (!admin) {
    return (
      <p role="status" className="mb-3 rounded-md border p-3 text-sm">
        Waiting for an admin to trust this swarm.
      </p>
    );
  }
  const refresh = () => qc.invalidateQueries({ queryKey: ["env", env, "swarm"] });
  return (
    <section aria-label="Trust this swarm" className="mb-4 grid gap-3 rounded-md border border-primary/40 bg-cord-soft p-4 text-sm">
      <p>
        <strong>{t.pinned_by ?? "A manager"}</strong> reports swarm <strong>{t.cluster_id}</strong>. Before FleetOps trusts its agents, check the
        root CA on a manager:
      </p>
      <pre className="overflow-x-auto rounded bg-muted p-2 font-mono text-xs">{t.check_command}</pre>
      <p>
        It should print this fingerprint:
        <code className="mt-1 block font-mono text-xs break-all">{t.fingerprint}</code>
      </p>
      <div className="flex gap-2">
        <Button
          onClick={async () => {
            try {
              await trustSwarm({ path: { env }, body: { cluster_id: t.cluster_id, fingerprint: t.fingerprint }, throwOnError: true });
              toast.success("Swarm trusted; its agents are let in as they reconnect");
            } catch (e) {
              toast.error(errorMessage(e));
            }
            await refresh();
          }}
        >
          Trust this swarm
        </Button>
        <Button variant="outline" onClick={() => setReject(true)}>
          Reject
        </Button>
      </div>
      {reject && (
        <ConfirmDialog
          open
          onOpenChange={(open) => !open && setReject(false)}
          title="Reject this swarm?"
          description="FleetOps forgets it, and the next manager agent to connect proposes a swarm again."
          confirmLabel="Reject"
          onConfirm={async () => {
            await retrustSwarm({ path: { env }, throwOnError: true });
            await refresh();
          }}
        />
      )}
    </section>
  );
}

/** A swarm's nodes, whether each has an agent, and the key and trust actions. */
export function NodesPage({ env }: { env: string }) {
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
  const [cols] = useState(() => nodeColumns(env));
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
      <TrustCard env={env} admin={user.role === "admin"} />
      {down && (
        <p role="status" className="mb-3 rounded-md border border-destructive/40 bg-destructive/5 p-3 text-sm">
          {down}
        </p>
      )}
      <DataTable label="Nodes" data={agents.data} error={agents.error} columns={cols} getRowId={(n) => n.node_id} empty="No nodes yet." />
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
