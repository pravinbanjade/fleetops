import type { TaskInfo } from "@/api/gen/types.gen";
import type { WsClient } from "./ws";

/** Progress messages kept per task; the oldest go first. */
export const TASK_MESSAGES_MAX = 5000;

export interface TrackedTask {
  id: string;
  env: string;
  /** What the user started, for the result toast: "Pull nginx:1.27". */
  label: string;
  /** Docker's progress messages, as the task topic sends them. */
  messages: unknown[];
  status: TaskInfo["status"];
  error?: string;
}

/**
 * Follows background tasks (image pull and build) over the websocket
 * `task` topic until they end, whether or not a dialog is still showing
 * them, and reports each result once through `onDone`.
 */
export class TaskTracker {
  private readonly ws: WsClient;
  private readonly onDone: (t: TrackedTask) => void;
  private readonly tasks = new Map<string, TrackedTask>();
  private readonly listeners = new Set<() => void>();

  constructor(ws: WsClient, onDone: (t: TrackedTask) => void) {
    this.ws = ws;
    this.onDone = onDone;
  }

  track(id: string, env: string, label: string) {
    if (this.tasks.has(id)) return;
    this.tasks.set(id, { id, env, label, messages: [], status: "running" });
    this.emit();
    const { unsubscribe } = this.ws.subscribe(
      { topic: "task", id },
      {
        onData: (d) =>
          this.update(id, (t) => ({ ...t, messages: [...t.messages, d].slice(-TASK_MESSAGES_MAX) })),
        // The server sends everything so far again on a new subscription.
        onResubscribe: () => this.update(id, (t) => ({ ...t, messages: [] })),
        onEnd: (m) => {
          const failed = m.reason !== "succeeded";
          this.finish(id, failed ? "failed" : "succeeded", failed ? (m.task?.error ?? `task ${m.reason}`) : undefined);
        },
        onError: (m) => {
          this.finish(id, "failed", m.message);
          unsubscribe();
        },
      },
    );
  }

  get(id: string): TrackedTask | undefined {
    return this.tasks.get(id);
  }

  /** For `useSyncExternalStore`. */
  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => void this.listeners.delete(listener);
  };

  private finish(id: string, status: TrackedTask["status"], error?: string) {
    const t = this.tasks.get(id);
    if (!t || t.status !== "running") return;
    this.update(id, (old) => ({ ...old, status, error }));
    this.onDone(this.tasks.get(id)!);
  }

  private update(id: string, f: (t: TrackedTask) => TrackedTask) {
    const t = this.tasks.get(id);
    if (!t) return;
    this.tasks.set(id, f(t));
    this.emit();
  }

  private emit() {
    for (const l of this.listeners) l();
  }
}
