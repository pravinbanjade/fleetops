import { useQuery } from "@tanstack/react-query";
import { useLocation, useNavigate } from "@tanstack/react-router";
import { useEffect, useMemo, useRef, useState } from "react";
import type { DockerEvent } from "@/api/gen/types.gen";
import { envStatusQuery, kubeListQuery, recentEventsQuery, swarmKeys } from "@/api/queries";
import type { EnvFamily } from "@/lib/env";
import { cn } from "@/lib/utils";
import { useKubeWatch } from "@/live/kubeWatch";
import { useSwarmEvents } from "@/live/swarmEvents";
import { useMedia } from "../useMedia";
import { useNow } from "./now";
import { capTape, dockerTapeItem, kubeTapeItem, type TapeItem } from "./tapeItems";

/** The tape redraws at most this often, however fast events arrive. */
const THROTTLE_MS = 250;

/**
 * The event tape along the foot of the window: the environment's events
 * streaming past, newest on the left, failures in red. Each item selects
 * its object when its list is on screen, else opens that list.
 */
export function Tape({ env, family }: { env: string; family: EnvFamily }) {
  const { data: status } = useQuery(envStatusQuery(env, family === "kube" || family === "swarm"));
  if (status?.state === "down") return <Strip env={env} items="down" />;
  if (family === "kube") return <KubeTape env={env} />;
  if (family === "swarm") return <SwarmTape env={env} />;
  return <DockerTape env={env} />;
}

function DockerTape({ env }: { env: string }) {
  const { data } = useQuery(recentEventsQuery(env));
  return <Strip env={env} items={fromDocker(data)} />;
}

function SwarmTape({ env }: { env: string }) {
  useSwarmEvents(env);
  const { data } = useQuery<DockerEvent[]>({ queryKey: swarmKeys.events(env), queryFn: () => [], staleTime: Infinity });
  return <Strip env={env} items={fromDocker(data)} />;
}

function KubeTape({ env }: { env: string }) {
  // Warnings only: a busy cluster sends Normal events every second, and the
  // overview already watches this filtered list, so the watch is shared.
  const live = useKubeWatch(env, "events", undefined, undefined, "warnings");
  const { data } = useQuery({ ...kubeListQuery(env, "events", undefined, "warnings"), enabled: !live.synced });
  return <Strip env={env} items={capTape((data?.rows ?? []).map(kubeTapeItem))} />;
}

function fromDocker(events: DockerEvent[] | undefined): TapeItem[] {
  return capTape((events ?? []).flatMap((ev) => dockerTapeItem(ev) ?? []));
}

/** `value`, but changing at most once per `ms`. */
function useThrottled<T>(value: T, ms: number): T {
  const [shown, setShown] = useState(value);
  const last = useRef(0);
  useEffect(() => {
    const wait = Math.max(0, last.current + ms - Date.now());
    const t = setTimeout(() => {
      last.current = Date.now();
      setShown(value);
    }, wait);
    return () => clearTimeout(t);
  }, [value, ms]);
  return shown;
}

function Strip({ env, items: incoming }: { env: string; items: TapeItem[] | "down" }) {
  const items = useThrottled(incoming, THROTTLE_MS);
  const phone = useMedia("(max-width: 767px)");
  const navigate = useNavigate();
  const { pathname } = useLocation();
  const { open } = useNow();
  const shown = useMemo(() => (items === "down" ? [] : phone ? items.slice(0, 1) : items), [items, phone]);
  const go = (t: NonNullable<TapeItem["target"]>) => {
    const href = Object.entries({ env, ...t.params }).reduce((h, [k, v]) => h.replace(`$${k}`, encodeURIComponent(v)), t.to);
    if (pathname === href && t.select) open(t.select);
    else void navigate({ to: t.to as never, params: { env, ...t.params } as never });
  };
  return (
    <div role="log" aria-label="Events" aria-live="off" className="flex h-[34px] min-w-0 items-center gap-6 overflow-hidden px-3.5 text-[12.5px] whitespace-nowrap text-muted-foreground">
      <span className="flex shrink-0 items-center gap-1.5 font-medium text-live">
        <span aria-hidden className="size-1.5 rounded-full bg-live" />
        Live
      </span>
      {items === "down" ? (
        <span className="text-meta">Environment unreachable</span>
      ) : shown.length === 0 ? (
        <span className="text-meta">No events yet</span>
      ) : (
        shown.map((it) => (
          <button
            key={it.key}
            type="button"
            disabled={!it.target}
            onClick={() => it.target && go(it.target)}
            className={cn(
              "flex shrink-0 items-center gap-1.5 rounded-sm press hover:text-foreground disabled:cursor-default motion-safe:animate-in motion-safe:fade-in-0",
              it.failing && "text-fall hover:text-fall",
            )}
          >
            <time className="font-figure text-foreground">{clock(it.at)}</time>
            {it.text}
          </button>
        ))
      )}
    </div>
  );
}

const clock = (unix: number) => new Date(unix * 1000).toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit", second: "2-digit", hour12: false });
