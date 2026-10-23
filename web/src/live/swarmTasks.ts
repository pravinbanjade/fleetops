import { useState } from "react";
import type { TaskSummary } from "@/api/gen/types.gen";
import { useSubscription } from "./LiveProvider";

/** What the `swarm-tasks` feed sends (swarm spec §7). */
type FeedMsg =
  | { type: "snapshot"; tasks: TaskSummary[] }
  | { type: "changes"; upsert: TaskSummary[]; remove: string[] }
  | { type: "paused"; reason: string };

/** The task list after one feed message; changes before a snapshot are dropped (one follows). */
export function applyFeed(tasks: TaskSummary[] | undefined, msg: FeedMsg): TaskSummary[] | undefined {
  switch (msg.type) {
    case "snapshot":
      return msg.tasks;
    case "paused":
      return tasks;
    case "changes": {
      if (!tasks) return tasks;
      const gone = new Set([...msg.remove, ...msg.upsert.map((t) => t.id)]);
      return [...tasks.filter((t) => !gone.has(t.id)), ...msg.upsert];
    }
  }
}

/**
 * A swarm's tasks, live from the shared feed. `paused` says why the list
 * is not current (the server could not read it, or the feed ended); the
 * tasks shown are then the last ones read.
 */
export function useSwarmTasks(env: string) {
  const [state, setState] = useState<{ env: string; tasks?: TaskSummary[]; paused: string | null }>({ env, paused: null });
  const current = state.env === env ? state : { env, paused: null };
  useSubscription(
    { topic: "swarm-tasks", env },
    {
      onData: (d) => {
        const msg = d as FeedMsg;
        setState((s) => {
          const base = s.env === env ? s : { env, paused: null };
          return { env, tasks: applyFeed(base.tasks, msg), paused: msg.type === "paused" ? msg.reason : null };
        });
      },
      onEnd: () => setState((s) => ({ ...s, paused: "live updates ended" })),
      onError: (m) => setState((s) => ({ ...s, paused: m.message })),
    },
  );
  return { tasks: current.tasks, paused: current.paused };
}
