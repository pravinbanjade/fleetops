import type { KubeCondition, KubeEvent } from "@/api/gen/types.gen";
import { ListChecks } from "lucide-react";
import { Panel } from "@/components/Panel";
import { StateBadge, type Tone } from "@/components/StateBadge";
import { errorMessage } from "@/lib/errors";
import { formatAgo } from "@/lib/format";

/** Conditions whose True is bad news; for the rest True is the healthy reading. */
const BAD_WHEN_TRUE = new Set(["MemoryPressure", "DiskPressure", "PIDPressure", "NetworkUnavailable", "Failed", "ReplicaFailure", "Stalled"]);

function conditionTone(c: KubeCondition): Tone {
  if (c.status === "Unknown") return "idle";
  const good = (c.status === "True") !== BAD_WHEN_TRUE.has(c.type);
  return good ? "run" : "fail";
}

/** Each condition as a strip: its reading as a holder, the type, and why. */
export function Conditions({ conditions }: { conditions: KubeCondition[] }) {
  return (
    <Panel title="Conditions" icon={ListChecks} flush>
      <ul className="font-strip text-sm">
        {conditions.map((c) => (
          <li key={c.type} className="grid grid-cols-[4.75rem_minmax(0,1fr)] items-baseline gap-x-3 gap-y-0.5 border-b px-4 py-2.5 last:border-b-0 sm:grid-cols-[4.75rem_minmax(0,14rem)_minmax(0,1fr)]">
            <StateBadge state={c.status} tone={conditionTone(c)} track={c.type} />
            <span className="font-ident font-medium">{c.type}</span>
            <span className="col-start-2 min-w-0 text-muted-foreground sm:col-start-auto">
              {c.reason && <span className="font-ident text-foreground">{c.reason}</span>}
              {c.reason && c.message && " · "}
              {c.message}
            </span>
          </li>
        ))}
      </ul>
    </Panel>
  );
}

/** An object's events, newest first, in the strips the overview uses. */
export function EventStrips({ events, pending, error }: { events: KubeEvent[] | undefined; pending: boolean; error: unknown }) {
  if (error) return <p className="text-sm text-destructive">{errorMessage(error)}</p>;
  if (pending) return <p className="text-sm text-muted-foreground">Loading events…</p>;
  if (!events?.length) return <p className="text-sm text-muted-foreground">No recent events.</p>;
  return (
    <ul aria-label="Events" className="font-strip overflow-hidden rounded-lg bg-raised text-sm">
      {events.map((e, i) => {
        const when = [e.count > 1 && `×${e.count}`, e.last_seen && formatAgo(e.last_seen)].filter(Boolean).join(" · ");
        return (
          <li key={i} className="grid grid-cols-[auto_minmax(0,1fr)] items-center gap-x-3 gap-y-0.5 border-b px-4 py-2.5 last:border-b-0 sm:grid-cols-[auto_minmax(0,1fr)_auto]">
            <StateBadge state={e.reason || e.type} tone={e.type === "Warning" ? "wait" : "idle"} />
            <span className="min-w-0 break-words">{e.message}</span>
            <span className="hidden text-xs whitespace-nowrap text-muted-foreground tabular-nums sm:inline">{when}</span>
            {/* On a phone the count and age join the source line: the message keeps the width. */}
            <span className="col-start-2 text-xs text-muted-foreground sm:col-end-4">
              {e.source && <span className="font-ident">{e.source}</span>}
              <span className="tabular-nums sm:hidden">{e.source && when && " · "}{when}</span>
            </span>
          </li>
        );
      })}
    </ul>
  );
}
