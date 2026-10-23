import { useParams } from "@tanstack/react-router";
import { Link } from "@tanstack/react-router";
import type { TaskSummary } from "@/api/gen/types.gen";
import { cn } from "@/lib/utils";

/** The environment of the current route. */
export function useEnv(): string {
  return useParams({ strict: false }).env ?? "";
}

/** Task states that are over: they never come back. */
export const TERMINAL = new Set(["complete", "shutdown", "failed", "rejected", "orphaned", "remove"]);

export const failed = (t: TaskSummary) => t.state === "failed" || t.state === "rejected";

/** A task's state with its colour: running, waiting, failed or done. */
export function TaskState({ task }: { task: TaskSummary }) {
  const tone = task.state === "running" ? "bg-live" : failed(task) ? "bg-state-fail" : TERMINAL.has(task.state) ? "bg-muted-foreground" : "bg-state-wait";
  return (
    <span className="inline-flex items-center gap-1.5">
      <span aria-hidden className={cn("size-2 rounded-full", tone)} />
      {task.state}
    </span>
  );
}

/** `<service>.<slot>`, or the service alone for global tasks. */
export const taskName = (t: TaskSummary) => (t.slot ? `${t.service_name}.${t.slot}` : t.service_name);

/** A link to the task's container on its node's Docker pages, when it has one. */
export function TaskContainer({ env, task }: { env: string; task: TaskSummary }) {
  if (!task.container_id || !task.node_id) return null;
  return (
    <Link
      to="/envs/$env/containers/$id"
      params={{ env: `${env}@${task.node_id}`, id: task.container_id }}
      className="font-mono text-xs hover:underline"
    >
      {task.container_id.slice(0, 12)}
    </Link>
  );
}
