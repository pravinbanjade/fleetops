import { cn } from "@/lib/utils";

/**
 * A reading and its share of capacity as a hairline track. The track takes
 * a state hue only as a reading of capacity: wait past 75%, fail past 90%.
 * `plain` is for a share of something other than capacity (a ranking).
 */
export function Meter({ value, share, title, plain, className }: { value: string; share?: number; title?: string; plain?: boolean; className?: string }) {
  return (
    <span className={cn("grid min-w-0 gap-1 tabular-nums", className)} title={title}>
      <span>{value}</span>
      {share !== undefined && (
        <span aria-hidden className="h-0.5 overflow-hidden rounded-full bg-muted">
          <span
            className={cn(
              "block h-full rounded-full transition-[width] duration-700 ease-out",
              plain ? "bg-foreground/60" : share > 0.9 ? "bg-state-fail" : share > 0.75 ? "bg-state-wait" : "bg-foreground/60",
            )}
            style={{ width: `${Math.min(100, share * 100).toFixed(1)}%` }}
          />
        </span>
      )}
    </span>
  );
}
