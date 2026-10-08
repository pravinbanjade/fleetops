import { cn } from "@/lib/utils";

type State = "ready" | "running" | "waiting" | "error" | "done" | "unknown";

/** One container out of a pod row's `containers` column. */
export interface ContainerSquare {
  init: boolean;
  name: string;
  state: State;
  reason: string;
}

const STATES: readonly string[] = [
  "ready",
  "running",
  "waiting",
  "error",
  "done",
  "unknown",
];

/** Reads the `kind:name:state:reason;…` column the server writes for a pod. */
export function parseContainers(column: string | undefined): ContainerSquare[] {
  if (!column) return [];
  return column.split(";").flatMap((entry) => {
    const [kind, name, state, ...rest] = entry.split(":");
    if (!name) return [];
    return [
      {
        init: kind === "i",
        name,
        state: (STATES.includes(state) ? state : "unknown") as State,
        reason: rest.join(":"),
      },
    ];
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

const label = (c: ContainerSquare) =>
  `${c.name}${c.init ? " (init)" : ""}: ${c.state}${c.reason ? `, ${c.reason}` : ""}`;

/** A pod's containers as a row of squares, init containers first and fainter. */
export function ContainerSquares({
  column,
  ready,
}: {
  column?: string;
  ready?: string;
}) {
  const all = parseContainers(column);
  if (all.length === 0)
    return <span className="text-muted-foreground">{ready ?? "—"}</span>;
  const [up, of] = (ready ?? "").split("/");
  return (
    <span
      role="group"
      aria-label={`Containers: ${up ?? 0} of ${of ?? all.length} ready`}
      title={ready}
      className="inline-flex items-center gap-1"
    >
      {all.map((c) => (
        <span
          key={`${c.init ? "i" : "c"}:${c.name}`}
          role="img"
          aria-label={label(c)}
          title={label(c)}
          data-state={c.state}
          className={cn(
            "size-2.5 shrink-0 rounded-[2px] border",
            FILL[c.state],
            c.init && "opacity-55",
          )}
        />
      ))}
    </span>
  );
}

/** How many containers are ready, from the `ready/want` column: what the squares sort by. */
export const readyCount = (ready: string | undefined) =>
  Number((ready ?? "").split("/")[0]) || 0;
