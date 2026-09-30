import uPlot from "uplot";
import "uplot/dist/uPlot.min.css";
import { useEffect, useRef } from "react";

export interface ChartSeries {
  label: string;
  /** A concrete CSS colour: the chart draws on a canvas, which cannot read CSS variables. */
  color: string;
  values: (number | null)[];
}

const HEIGHT = 160;
// Neutral enough to read on the light and the dark background.
const AXIS = "#8a8a8a";
const GRID = "rgba(138, 138, 138, 0.18)";

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
        legend: { show: true },
        cursor: { drag: { x: false, y: false } },
        scales: { x: { time: true }, y: { range: (_u, _min, max) => [0, max > 0 ? max * 1.1 : 1] } },
        axes: [
          { stroke: AXIS, grid: { stroke: GRID }, ticks: { stroke: GRID } },
          {
            stroke: AXIS,
            grid: { stroke: GRID },
            ticks: { stroke: GRID },
            size: 72,
            values: (_u, vals) => vals.map((v) => fmt(v)),
          },
        ],
        series: [
          {},
          ...series.map((s) => ({
            label: s.label,
            stroke: s.color,
            width: 1.5,
            fill: `${s.color}22`,
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
    return () => {
      ro.disconnect();
      u.destroy();
      plot.current = null;
    };
  }, []);

  useEffect(() => {
    plot.current?.setData([times, ...series.map((s) => s.values)]);
  }, [times, series]);

  return (
    <figure className="grid gap-1">
      <figcaption className="text-sm font-medium">{title}</figcaption>
      <div ref={box} role="img" aria-label={`${title} chart`} className="min-w-0" />
    </figure>
  );
}
