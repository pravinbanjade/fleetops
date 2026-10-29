import type { LucideIcon } from "lucide-react";
import type { CSSProperties } from "react";
import { Sparkline } from "@/components/Sparkline";
import { Tick } from "@/components/Tick";

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
  window,
}: {
  label: string;
  icon: LucideIcon;
  value: string;
  detail: string;
  share?: number;
  values: number[];
  max: number;
  span?: string;
  /** Readings the line's full width holds, for a slow series; see `Sparkline`. */
  window?: number;
}) {
  return (
    <div
      role="group"
      aria-label={label}
      className="grid min-w-0 gap-1 rounded-lg bg-raised px-3 py-2.5 sm:w-[12rem] xl:w-auto"
    >
      <div className="flex items-center gap-3">
        <span className="flex shrink-0 items-center gap-1.5 text-xs font-medium text-muted-foreground">
          <Icon aria-hidden className="size-3.5" />
          {label}
        </span>
        {/* Up to 96px, and never wider than what the label leaves. */}
        <span className="ml-auto min-w-0 flex-1 basis-0 sm:max-w-24">
          <Sparkline values={values} max={max} label={`${label} over ${span}`} width={96} height={20} window={window} minPoints={window ? 3 : undefined} fluid />
        </span>
      </div>
      <p className="min-w-0 truncate tabular-nums">
        <Tick className="text-lg font-semibold" value={values.at(-1) ?? 0} text={value} />{" "}
        <span className="text-xs whitespace-nowrap text-meta">{detail}</span>
      </p>
      {share !== undefined && (
        <div aria-hidden className="h-0.5 overflow-hidden rounded-full bg-muted">
          <div className="bar-fill bg-foreground/60" style={{ "--share": share } as CSSProperties} />
        </div>
      )}
    </div>
  );
}
