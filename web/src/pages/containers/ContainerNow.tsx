import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { ArrowUpRight, Play, RotateCw, Square, SquareTerminal } from "lucide-react";
import { toast } from "sonner";
import { useEnvAccess } from "@/api/access";
import { containerAction } from "@/api/gen/sdk.gen";
import type { ContainerActionName, ContainerSummary, DockerEvent } from "@/api/gen/types.gen";
import { containerQuery, keys, recentEventsQuery } from "@/api/queries";
import { LogTail } from "@/components/LogTail";
import { NowPanel } from "@/components/shell/now";
import { Tick } from "@/components/Tick";
import { useClock } from "@/components/useClock";
import { Button } from "@/components/ui/button";
import { formatBytes } from "@/lib/format";
import { cn } from "@/lib/utils";
import { useContainerStats } from "@/live/LiveProvider";
import { actionsFor } from "./actions";

/** How far back the restart count looks, in seconds. */
const WINDOW = 600;

/** How many times container `id` died in the `window` seconds before `now` (unix seconds). */
export function restartsWithin(events: Pick<DockerEvent, "action" | "id" | "time">[], id: string, now: number, window = WINDOW): number {
  return events.filter((e) => e.id === id && e.action === "die" && now - e.time <= window).length;
}

/** "Exited (137) 2 minutes ago" → 137. */
const exitCode = (status: string) => /\((\d+)\)/.exec(status)?.[1];

/**
 * A container in the "now playing" panel: what state it is in, how often it
 * restarted lately, its memory against its limit, its live log, and the
 * actions an operator reaches for first.
 */
export function ContainerNow({ env, container: c }: { env: string; container: ContainerSummary }) {
  const { can } = useEnvAccess(env);
  const qc = useQueryClient();
  const detail = useQuery(containerQuery(env, c.id));
  const events = useQuery(recentEventsQuery(env));
  const stats = useContainerStats(env, c.id, c.state === "running");
  const last = stats.at(-1);
  const clock = useClock();
  const restarts = restartsWithin(events.data ?? [], c.id, clock);
  const code = c.state === "running" ? undefined : exitCode(c.status);
  const allowed = actionsFor(c.state);
  const act = useMutation({
    mutationFn: async (action: ContainerActionName) => {
      await containerAction({ path: { env, id: c.id, action }, throwOnError: true });
      toast.success(`${action === "restart" ? "Restarted" : action === "stop" ? "Stopped" : "Started"} ${c.name}`);
    },
    onSettled: () => qc.invalidateQueries({ queryKey: keys.containers(env) }),
  });
  const operate = can("containers.operate");
  const failing = c.state !== "running" && !!code && code !== "0";
  return (
    <NowPanel
      title={<span className={cn("font-ident text-[17px]", failing && "text-fall")}>{c.name}</span>}
      subtitle={
        <>
          <span className="font-ident">{c.image}</span> · <span className={cn(failing ? "text-fall" : c.state === "running" ? "text-state-run-text" : "text-muted-foreground")}>{c.state}{code && `, exit ${code}`}</span>
        </>
      }
    >
      <div className="grid grid-cols-3 gap-2">
        <Fact label="Restarts, 10 min">
          <Tick value={restarts} text={String(restarts)} className={cn(restarts > 0 && "text-fall")} />
        </Fact>
        <Fact label="Memory">{last ? <Tick value={last.memory_used} text={formatBytes(last.memory_used)} /> : <span className="text-meta">—</span>}</Fact>
        <Fact label="Limit">{last?.memory_limit ? <span className="font-figure">{formatBytes(last.memory_limit)}</span> : <span className="text-meta">—</span>}</Fact>
      </div>
      <LogTail env={env} id={c.id} startedAt={detail.data?.State.StartedAt} />
      <div className="flex flex-wrap gap-2">
        <Button asChild size="sm">
          <Link to="/envs/$env/containers/$id" params={{ env, id: c.id }}>
            <ArrowUpRight />
            Open
          </Link>
        </Button>
        {can("containers.shell") && c.state === "running" && (
          <Button asChild size="sm" variant="secondary">
            <Link to="/envs/$env/containers/$id" params={{ env, id: c.id }} search={{ tab: "terminal" }}>
              <SquareTerminal />
              Shell
            </Link>
          </Button>
        )}
        {operate && allowed.includes("restart") && (
          <Button size="sm" variant="secondary" disabled={act.isPending} onClick={() => act.mutate("restart")}>
            <RotateCw />
            Restart
          </Button>
        )}
        {operate && allowed.includes("start") && (
          <Button size="sm" variant="secondary" disabled={act.isPending} onClick={() => act.mutate("start")}>
            <Play />
            Start
          </Button>
        )}
        {operate && allowed.includes("stop") && (
          <Button size="sm" variant="secondary" disabled={act.isPending} onClick={() => act.mutate("stop")}>
            <Square />
            Stop
          </Button>
        )}
      </div>
    </NowPanel>
  );
}

function Fact({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="grid gap-0.5 rounded-md bg-[#1c1c1c] px-2.5 py-2">
      <span className="truncate text-xs text-meta">{label}</span>
      <span className="text-base font-semibold">{children}</span>
    </div>
  );
}
