import { useQuery } from "@tanstack/react-query";
import { useMemo } from "react";
import type { TaskSummary } from "@/api/gen/types.gen";
import { swarmNodesQuery } from "@/api/queries";
import { columns, DataTable, type Column } from "@/components/DataTable";
import { Page } from "@/components/Page";
import { formatAgo } from "@/lib/format";
import { useSwarmTasks } from "@/live/swarmTasks";
import { TaskContainer, TaskState, taskName, useEnv } from "./shared";

const col = columns<TaskSummary>();

/** A node's hostname, from the cached node list. */
function NodeName({ env, id }: { env: string; id?: string | null }) {
  const nodes = useQuery(swarmNodesQuery(env));
  return <>{nodes.data?.find((n) => n.id === id)?.hostname ?? id ?? ""}</>;
}

/** A task table. */
export function taskColumns(env: string): Column<TaskSummary>[] {
  return [
    col.accessor(taskName, { id: "task", header: "Task", cell: (c) => <span className="font-medium">{c.getValue()}</span> }),
    col.accessor("state", { header: "State", cell: (c) => <TaskState task={c.row.original} /> }),
    col.accessor("desired_state", { header: "Desired" }),
    col.accessor((t) => t.node_id ?? "", { id: "node", header: "Node", cell: (c) => <NodeName env={env} id={c.row.original.node_id} /> }),
    col.accessor((t) => t.container_id ?? "", { id: "container", header: "Container", cell: (c) => <TaskContainer env={env} task={c.row.original} /> }),
    col.accessor((t) => t.error ?? t.message, { id: "message", header: "Message", cell: (c) => <span className={c.row.original.error ? "text-destructive" : undefined}>{c.getValue()}</span> }),
    col.accessor("updated_at", { header: "Updated", cell: (c) => formatAgo(c.getValue()) }),
  ];
}

/** Paused live data, said plainly. */
export function FeedPaused({ reason }: { reason: string | null }) {
  if (!reason) return null;
  return (
    <p role="status" className="mb-3 rounded-md border border-state-wait/40 bg-state-wait/10 px-3 py-2 text-sm">
      Live updates paused — {reason}
    </p>
  );
}

export function TasksPage() {
  const env = useEnv();
  const { tasks, paused } = useSwarmTasks(env);
  const cols = useMemo(() => taskColumns(env), [env]);
  return (
    <Page title="Tasks">
      <FeedPaused reason={paused} />
      <DataTable label="Tasks" data={tasks} columns={cols} getRowId={(t) => t.id} empty="No tasks." initialSorting={[{ id: "updated_at", desc: true }]} />
    </Page>
  );
}
