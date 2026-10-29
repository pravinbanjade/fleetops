import { withAlpha } from "@/lib/color";
import uPlot from "uplot";
import "uplot/dist/uPlot.min.css";
import { useEffect, useRef } from "react";

export interface ChartSeries {
  label: string;
  /**
   * The series' ink: a chart's first series in the text colour, a second one
   * in the secondary text colour, dashed. State hues stay off charts: a
   * metric is not a reading of health.
   */
  tone: "primary" | "secondary";
  values: (number | null)[];
}

const HEIGHT = 160;
const FONT = "11px 'JetBrains Mono Variable', ui-monospace, monospace";

/**
 * A theme colour for the canvas. A canvas cannot read CSS variables, so the
 * variable is resolved when the chart draws; `alpha` fades it.
 */
function ink(name: "--foreground" | "--muted-foreground" | "--border" | "--meta" | "--live", alpha?: number): string {
  const v = getComputedStyle(document.documentElement).getPropertyValue(name).trim() || "#8a8a8a";
  return alpha === undefined ? v : withAlpha(v, alpha);
}

/**
 * A live time-series chart (uPlot). `times` are Unix seconds. The chart is
 * made once; new data is drawn into it, and it follows its box's width.
 * The y axis starts at zero.
 */
export function Chart({
  title,
  times,
  series,
  format,
}: {
  title: string;
  times: number[];
  series: ChartSeries[];
  format(v: number): string;
}) {
  const box = useRef<HTMLDivElement>(null);
  const plot = useRef<uPlot | null>(null);
  const latest = useRef({ times, series, format });
  useEffect(() => {
    latest.current = { times, series, format };
  });

  useEffect(() => {
    const el = box.current!;
    const { times, series, format } = latest.current;
    const fmt = (v: number | null) => (v == null ? "—" : format(v));
    const u = new uPlot(
      {
        width: el.clientWidth || 600,
        height: HEIGHT,
        legend: { show: true, live: true },
        cursor: { drag: { x: false, y: false } },
        scales: { x: { time: true }, y: { range: (_u, _min, max) => [0, max > 0 ? max * 1.1 : 1] } },
        axes: [
          { stroke: () => ink("--meta"), font: FONT, grid: { stroke: () => ink("--border") }, ticks: { stroke: () => ink("--border") } },
          {
            stroke: () => ink("--muted-foreground"),
            font: FONT,
            grid: { stroke: () => ink("--border") },
            ticks: { stroke: () => ink("--border") },
            size: 72,
            values: (_u, vals) => vals.map((v) => fmt(v)),
          },
        ],
        series: [
          {},
          ...series.map((s) => ({
            label: s.label,
            stroke: () => ink(s.tone === "primary" ? "--live" : "--muted-foreground"),
            width: 1.5,
            dash: s.tone === "secondary" ? [4, 3] : undefined,
            fill: s.tone === "primary" ? () => ink("--live", 0.1) : undefined,
            value: (_u: uPlot, v: number | null) => fmt(v),
          })),
        ],
      },
      [times, ...series.map((s) => s.values)],
      el,
    );
    plot.current = u;
    const ro = new ResizeObserver(() => u.setSize({ width: el.clientWidth || 600, height: HEIGHT }));
    ro.observe(el);
    // A theme switch repaints the canvas in the new theme's inks.
    const mo = new MutationObserver(() => u.redraw(false));
    mo.observe(document.documentElement, { attributes: true, attributeFilter: ["class"] });
    return () => {
      mo.disconnect();
      ro.disconnect();
      u.destroy();
      plot.current = null;
    };
  }, []);

  useEffect(() => {
    plot.current?.setData([times, ...series.map((s) => s.values)]);
  }, [times, series]);

  return (
    <figure className="grid gap-2 rounded-lg bg-raised p-4">
      <figcaption className="text-sm font-semibold">{title}</figcaption>
      <div ref={box} role="img" aria-label={`${title} chart`} className="min-w-0" />
    </figure>
  );
}
