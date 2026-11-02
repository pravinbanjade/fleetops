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
import { Fact } from "@/components/shell/ObjectNow";
import { Tick } from "@/components/Tick";
import { useClock } from "@/components/useClock";
import { Button } from "@/components/ui/button";
import { formatBytes } from "@/lib/format";
import { cn } from "@/lib/utils";
import { useContainerStats } from "@/live/LiveProvider";
import { RECENT_EVENTS } from "@/live/noise";
import { actionsFor } from "./actions";

/** How far back the restart count looks, in seconds. */
const WINDOW = 600;

/** A die this soon after a kill or stop was asked for, not a crash. */
const ASKED = 30;

/**
 * How many times container `id` died on its own in the `window` seconds
 * before `now` (unix seconds). A die that follows a kill or stop (Restart,
 * Stop, `docker restart`) was asked for and does not count. `events` is
 * newest first.
 */
export function restartsWithin(events: Pick<DockerEvent, "action" | "id" | "time">[], id: string, now: number, window = WINDOW): number {
  const mine = events.filter((e) => e.id === id);
  return mine.filter(
    (e, i) =>
      e.action === "die" &&
      now - e.time <= window &&
      !mine.slice(i + 1).some((p) => (p.action === "kill" || p.action === "stop") && e.time - p.time <= ASKED),
  ).length;
}

/**
 * Whether the feed reaches back over the whole window: it keeps the newest
 * RECENT_EVENTS across the host, so on a busy host the count is a floor.
 */
export const coversWindow = (events: Pick<DockerEvent, "time">[], now: number, window = WINDOW) =>
  events.length < RECENT_EVENTS || now - events[events.length - 1].time >= window;

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
  const floor = !coversWindow(events.data ?? [], clock);
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
          <Tick value={restarts} text={floor ? `≥${restarts}` : String(restarts)} className={cn(restarts > 0 && "text-fall")} />
        </Fact>
        <Fact label="Memory">{last ? <Tick value={last.memory_used} text={formatBytes(last.memory_used)} /> : <span className="text-meta">—</span>}</Fact>
        <Fact label="Limit">{last?.memory_limit ? <span className="font-figure">{formatBytes(last.memory_limit)}</span> : <span className="text-meta">—</span>}</Fact>
      </div>
      {/* Opened once the start time is known: a start time arriving later would restart the stream. */}
      {detail.isPending ? (
        <div className="flex min-h-0 flex-1 rounded-lg bg-[#0a0a0a] p-3 text-[12.5px] text-meta">Waiting for log lines…</div>
      ) : (
        <LogTail env={env} id={c.id} startedAt={detail.data?.State.StartedAt} />
      )}
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

