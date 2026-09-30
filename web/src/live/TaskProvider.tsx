import { useQueryClient } from "@tanstack/react-query";
import { createContext, useContext, useState, useSyncExternalStore, type ReactNode } from "react";
import { toast } from "sonner";
import { keys } from "@/api/queries";
import { useLive } from "./LiveProvider";
import { TaskTracker, type TrackedTask } from "./tasks";

const TaskContext = createContext<TaskTracker | null>(null);

/** Tracks the tab's background tasks; each result is a toast, and refreshes the image list. */
export function TaskProvider({ children }: { children: ReactNode }) {
  const { ws } = useLive();
  const qc = useQueryClient();
  const [tracker] = useState(
    () =>
      new TaskTracker(ws!, (t) => {
        void qc.invalidateQueries({ queryKey: keys.images(t.env) });
        if (t.status === "succeeded") toast.success(`${t.label}: done`);
        else toast.error(`${t.label} failed: ${t.error}`);
      }),
  );
  return <TaskContext value={tracker}>{children}</TaskContext>;
}

export function useTaskTracker() {
  const t = useContext(TaskContext);
  if (!t) throw new Error("useTaskTracker outside TaskProvider");
  return t;
}

/** A tracked task, re-rendering as its progress arrives. */
export function useTask(id: string | null): TrackedTask | undefined {
  const tracker = useTaskTracker();
  return useSyncExternalStore(tracker.subscribe, () => (id ? tracker.get(id) : undefined));
}
