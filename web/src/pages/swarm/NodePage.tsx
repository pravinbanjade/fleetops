import { useQuery } from "@tanstack/react-query";
import { Link, useParams } from "@tanstack/react-router";
import { useMemo } from "react";
import { swarmNodeQuery } from "@/api/queries";
import { DataTable } from "@/components/DataTable";
import { Page } from "@/components/Page";
import { errorMessage } from "@/lib/errors";
import { useSwarmTasks } from "@/live/swarmTasks";
import { NodeActions } from "./NodeActions";
import { FeedPaused, taskColumns } from "./TasksPage";

const gib = (b: number) => `${(b / 1024 ** 3).toFixed(1)} GiB`;

/** One node: what it is, its labels, the tasks on it, and a way into its Docker pages. */
export function NodePage() {
  const { env = "", id = "" } = useParams({ strict: false }) as { env?: string; id?: string };
  const node = useQuery(swarmNodeQuery(env, id));
  const { tasks, paused } = useSwarmTasks(env);
  const cols = useMemo(() => taskColumns(env), [env]);
  const n = node.data;
  if (node.error) return <Page title={id}>{<p role="alert" className="text-sm text-destructive">{errorMessage(node.error)}</p>}</Page>;
  return (
    <Page
      title={n?.hostname ?? id}
      meta={n && `${n.role}${n.leader ? " (leader)" : ""} · ${n.state} · ${n.availability} · Docker ${n.engine} · ${n.os}/${n.arch}`}
      actions={
        <span className="flex items-center gap-2">
          <Link to="/envs/$env" params={{ env: `${env}@${id}` }} className="text-sm underline">
            Containers, images and volumes on this node
          </Link>
          {n && <NodeActions env={env} node={n} />}
        </span>
      }
    >
      {n && (
        <dl className="mb-4 grid grid-cols-[max-content_1fr] gap-x-4 gap-y-1 text-sm">
          <dt className="text-muted-foreground">Address</dt>
          <dd>{n.addr}</dd>
          <dt className="text-muted-foreground">CPUs</dt>
          <dd>{(n.nano_cpus / 1e9).toFixed(1)}</dd>
          <dt className="text-muted-foreground">Memory</dt>
          <dd>{gib(n.memory_bytes)}</dd>
          {n.reachability && (
            <>
              <dt className="text-muted-foreground">Manager</dt>
              <dd>
                {n.reachability} {n.manager_addr && `· ${n.manager_addr}`}
              </dd>
            </>
          )}
          <dt className="text-muted-foreground">Labels</dt>
          <dd className="font-mono text-xs">{Object.entries(n.labels).map(([k, v]) => `${k}=${v}`).join(", ") || "none"}</dd>
        </dl>
      )}
      <FeedPaused reason={paused} />
      <DataTable label="Tasks" data={tasks?.filter((t) => t.node_id === id)} columns={cols} getRowId={(t) => t.id} empty="No tasks on this node." />
    </Page>
  );
}
