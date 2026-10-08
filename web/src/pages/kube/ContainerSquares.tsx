import { Tooltip } from "radix-ui";
import { cn } from "@/lib/utils";

type State = "ready" | "running" | "waiting" | "error" | "done" | "unknown";

/** One container out of a pod row's `containers` column. */
export interface ContainerSquare {
  name: string;
  init: boolean;
  state: State;
  reason: string;
  message: string;
}

const STATES: readonly string[] = ["ready", "running", "waiting", "error", "done", "unknown"];

/** Reads the JSON array the server writes into a pod row's `containers` column. */
export function parseContainers(column: string | undefined): ContainerSquare[] {
  if (!column) return [];
  let raw: unknown;
  try {
    raw = JSON.parse(column);
  } catch {
    return [];
  }
  if (!Array.isArray(raw)) return [];
  return raw.flatMap((c) => {
    if (typeof c?.name !== "string") return [];
    const text = (v: unknown) => (typeof v === "string" ? v : "");
    return [{ name: c.name, init: c.init === true, state: (STATES.includes(c.state) ? c.state : "unknown") as State, reason: text(c.reason), message: text(c.message) }];
  });
}

const FILL: Record<State, string> = {
  ready: "border-state-run bg-state-run",
  running: "border-state-wait bg-state-wait",
  waiting: "border-state-wait bg-state-wait",
  error: "border-state-fail bg-state-fail",
  done: "border-muted-foreground/60 bg-muted-foreground/40",
  unknown: "border-muted-foreground/60 bg-transparent",
};

const STATE_TEXT: Record<State, string> = {
  ready: "text-state-run-text",
  running: "text-state-wait-text",
  waiting: "text-state-wait-text",
  error: "text-state-fail-text",
  done: "text-muted-foreground",
  unknown: "text-muted-foreground",
};

const label = (c: ContainerSquare) => `${c.name}${c.init ? " (init)" : ""}: ${c.state}${c.reason ? `, ${c.reason}` : ""}`;

/** What hovering a square shows: the container, its state, and why. */
function Card({ c }: { c: ContainerSquare }) {
  return (
    <div className="grid max-w-[min(36rem,90vw)] gap-1.5">
      <p className="flex items-baseline gap-2">
        <span className="font-ident font-semibold">{c.name}</span>
        {c.init && <span className="text-muted-foreground">init</span>}
        <span className={STATE_TEXT[c.state]}>{c.state}</span>
      </p>
      {c.reason && (
        <div>
          <p className="text-muted-foreground">Reason</p>
          <p>{c.reason}</p>
        </div>
      )}
      {c.message && (
        <div>
          <p className="text-muted-foreground">Message</p>
          <p className="break-words whitespace-pre-wrap">{c.message}</p>
        </div>
      )}
    </div>
  );
}

/** A pod's containers as a row of squares, init containers first and fainter. */
export function ContainerSquares({ column, ready }: { column?: string; ready?: string }) {
  const all = parseContainers(column);
  if (all.length === 0) return <span className="text-muted-foreground">{ready ?? "—"}</span>;
  const [up, of] = (ready ?? "").split("/");
  return (
    <Tooltip.Provider delayDuration={150} skipDelayDuration={0}>
      <span role="group" aria-label={`Containers: ${up ?? 0} of ${of ?? all.length} ready`} className="inline-flex items-center gap-1">
        {all.map((c) => (
          <Tooltip.Root key={`${c.init ? "i" : "c"}:${c.name}`}>
            <Tooltip.Trigger asChild>
              <span
                role="img"
                tabIndex={0}
                aria-label={label(c)}
                data-state-name={c.state}
                className={cn("size-2.5 shrink-0 rounded-[2px] border outline-none focus-visible:ring-2 focus-visible:ring-ring", FILL[c.state], c.init && "opacity-55")}
              />
            </Tooltip.Trigger>
            <Tooltip.Portal>
              <Tooltip.Content
                side="bottom"
                align="start"
                sideOffset={6}
                collisionPadding={8}
                className="z-50 rounded-md border bg-popover px-3 py-2 text-sm text-popover-foreground shadow-md"
              >
                <Card c={c} />
              </Tooltip.Content>
            </Tooltip.Portal>
          </Tooltip.Root>
        ))}
      </span>
    </Tooltip.Provider>
  );
}

/** How many containers are ready, from the `ready/want` column: what the squares sort by. */
export const readyCount = (ready: string | undefined) => Number((ready ?? "").split("/")[0]) || 0;
