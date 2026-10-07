import type { LucideIcon } from "lucide-react";
import { Sparkline } from "@/components/Sparkline";

/**
 * A live value with its recent history, and its share of capacity as a
 * hairline track. The line's scale follows `max`. `span` names the history
 * for screen readers, e.g. "the last minute".
 */
export function Readout({
  label,
  icon: Icon,
  value,
  detail,
  share,
  values,
  max,
  span = "the last minute",
}: {
  label: string;
  icon: LucideIcon;
  value: string;
  detail: string;
  share?: number;
  values: number[];
  max: number;
  span?: string;
}) {
  return (
    <div
      role="group"
      aria-label={label}
      className="font-strip grid min-w-0 gap-1 rounded-lg border bg-card px-3 py-2 sm:w-[12rem] xl:w-auto"
    >
      <div className="flex items-center justify-between gap-3">
        <span className="flex items-center gap-1.5 text-xs font-medium text-muted-foreground">
          <Icon aria-hidden className="size-3.5" />
          {label}
        </span>
        <Sparkline values={values} max={max} label={`${label} over ${span}`} width={96} height={20} />
      </div>
      <p className="min-w-0 truncate tabular-nums">
        <span className="text-lg font-semibold">{value}</span>{" "}
        <span className="text-xs whitespace-nowrap text-muted-foreground">{detail}</span>
      </p>
      {share !== undefined && (
        <div aria-hidden className="h-0.5 overflow-hidden rounded-full bg-muted">
          <div
            className="h-full rounded-full bg-foreground/60 transition-[width] duration-700 ease-out"
            style={{ width: `${Math.min(100, share * 100).toFixed(1)}%` }}
          />
        </div>
      )}
    </div>
  );
}
