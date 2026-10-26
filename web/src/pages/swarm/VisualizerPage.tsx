import { useQuery } from "@tanstack/react-query";
import { swarmNodesQuery } from "@/api/queries";
import { Page } from "@/components/Page";
import { cn } from "@/lib/utils";
import { useSwarmTasks } from "@/live/swarmTasks";
import { FeedPaused } from "./TasksPage";
import { TERMINAL, taskName, useEnv } from "./shared";

/** Every node as a column, with the tasks Swarm runs (or is starting) there, live. */
export function VisualizerPage() {
  const env = useEnv();
  const nodes = useQuery(swarmNodesQuery(env));
  const { tasks, paused } = useSwarmTasks(env);
  const live = (tasks ?? []).filter((t) => !TERMINAL.has(t.state) && t.desired_state !== "shutdown");
  return (
    <Page title="Visualizer">
      <FeedPaused reason={paused} />
      <div className="flex gap-3 overflow-x-auto pb-2">
        {(nodes.data ?? []).map((n) => (
          <section
            key={n.id}
            role="group"
            aria-label={n.hostname}
            className={cn("grid w-56 shrink-0 content-start gap-2 rounded-lg bg-[#1c1c1c] p-3", n.state !== "ready" && "opacity-60")}
          >
            <header className="grid">
              <span className="font-medium">{n.hostname}</span>
              <span className="text-xs text-muted-foreground">
                {n.role}
                {n.leader && " · leader"} · {n.availability}
                {n.state !== "ready" && ` · ${n.state}`}
              </span>
            </header>
            {live
              .filter((t) => t.node_id === n.id)
              .sort((a, b) => taskName(a).localeCompare(taskName(b)))
              .map((t) => (
                <div
                  key={t.id}
                  className={cn(
                    "rounded-md bg-[#1c1c1c] px-2 py-1.5 text-sm",
                    t.state === "running" ? "border-live/50 bg-live/10" : "border-state-wait/50 bg-state-wait/10 opacity-70",
                  )}
                >
                  <span className="block truncate">{taskName(t)}</span>
                  {t.state !== "running" && <span className="text-xs text-muted-foreground">{t.state}</span>}
                </div>
              ))}
          </section>
        ))}
      </div>
    </Page>
  );
}
