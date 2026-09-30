import { useMemo } from "react";
import { Chart, type ChartSeries } from "@/components/Chart";
import { Details } from "@/components/Details";
import { formatBytes, formatPercent } from "@/lib/format";
import { useContainerStats } from "@/live/LiveProvider";
import type { StatsPoint } from "@/live/stats";

const rate = (v: number) => `${formatBytes(v)}/s`;

/** Live CPU, memory, network and block I/O for one container, over the last two minutes. */
export function StatsTab({ env, id, running }: { env: string; id: string; running: boolean }) {
  const points = useContainerStats(env, id, running);
  const charts = useMemo(() => chartData(points), [points]);
  if (!running) return <p className="text-sm text-muted-foreground">The container is not running.</p>;
  const last = points.at(-1);
  if (!last) return <p className="text-sm text-muted-foreground">Waiting for the first sample…</p>;
  const memPct = last.memory_limit > 0 ? ` (${formatPercent((last.memory_used / last.memory_limit) * 100)})` : "";
  return (
    <div className="grid gap-6">
      <Details
        label="Current usage"
        items={[
          ["CPU", formatPercent(last.cpu_percent)],
          ["Memory", `${formatBytes(last.memory_used)} of ${formatBytes(last.memory_limit)}${memPct}`],
          ["Network", `${fmtRate(last.net_rx_rate)} in, ${fmtRate(last.net_tx_rate)} out`],
          ["Block I/O", `${fmtRate(last.block_read_rate)} read, ${fmtRate(last.block_write_rate)} written`],
          ["Processes", String(last.pids)],
        ]}
      />
      <div className="grid gap-6 lg:grid-cols-2">
        <Chart title="CPU" times={charts.times} series={charts.cpu} format={formatPercent} />
        <Chart title="Memory" times={charts.times} series={charts.memory} format={formatBytes} />
        <Chart title="Network" times={charts.times} series={charts.network} format={rate} />
        <Chart title="Block I/O" times={charts.times} series={charts.block} format={rate} />
      </div>
    </div>
  );
}

const fmtRate = (v: number | null) => (v === null ? "—" : rate(v));

function chartData(points: StatsPoint[]) {
  const col = (f: (p: StatsPoint) => number | null) => points.map(f);
  const series = (label: string, tone: ChartSeries["tone"], values: (number | null)[]): ChartSeries => ({
    label,
    tone,
    values,
  });
  return {
    times: points.map((p) => p.time / 1000),
    cpu: [series("CPU", "primary", col((p) => p.cpu_percent))],
    memory: [series("Used", "primary", col((p) => p.memory_used))],
    network: [
      series("In", "primary", col((p) => p.net_rx_rate)),
      series("Out", "secondary", col((p) => p.net_tx_rate)),
    ],
    block: [
      series("Read", "primary", col((p) => p.block_read_rate)),
      series("Written", "secondary", col((p) => p.block_write_rate)),
    ],
  };
}
