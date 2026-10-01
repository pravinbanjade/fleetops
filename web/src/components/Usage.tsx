import type { ContainerSummary } from "@/api/gen/types.gen";
import { Sparkline } from "@/components/Sparkline";
import { useInView } from "@/components/useInView";
import { formatBytes, formatPercent } from "@/lib/format";
import { useContainerStats } from "@/live/LiveProvider";

/**
 * Live CPU or memory for a running container: the latest value and a
 * sparkline of the last minute. Only rows on screen subscribe; the CPU and
 * memory cells of one row share a subscription.
 */
export function Usage({ env, container, kind }: { env: string; container: ContainerSummary; kind: "cpu" | "memory" }) {
  const [ref, inView] = useInView<HTMLDivElement>();
  const points = useContainerStats(env, container.id, container.state === "running" && inView);
  const last = points.at(-1);
  if (!last) {
    return (
      <div ref={ref} className="text-muted-foreground">
        —
      </div>
    );
  }
  if (kind === "cpu") {
    const values = points.map((p) => p.cpu_percent);
    return (
      <div ref={ref} className="flex items-center gap-2 tabular-nums">
        <Sparkline values={values} max={Math.max(100, ...values)} label={`CPU of ${container.name}`} />
        <span className="w-12 text-right">{formatPercent(last.cpu_percent)}</span>
      </div>
    );
  }
  const values = points.map((p) => p.memory_used);
  return (
    <div ref={ref} className="flex items-center gap-2 tabular-nums" title={`Limit ${formatBytes(last.memory_limit)}`}>
      <Sparkline values={values} max={last.memory_limit || Math.max(...values) * 1.2 || 1} label={`Memory of ${container.name}`} />
      <span className="w-16 text-right">{formatBytes(last.memory_used)}</span>
    </div>
  );
}
