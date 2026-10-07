import { useQuery } from "@tanstack/react-query";
import { useMemo, useState } from "react";
import type { KubeUsageRow } from "@/api/gen/types.gen";
import { ApiError } from "@/api/client";
import { kubeUsageQuery } from "@/api/queries";

export const usageKey = (ns: string | null | undefined, name: string) => `${ns ?? "-"}/${name}`;

/**
 * CPU and memory now for nodes or pods, indexed by `usageKey`. `reason`
 * says why there are none: the cluster has no metrics API (metrics-server
 * is not installed or not answering), or the credentials may not read it.
 */
export function useKubeUsage(env: string, kind: "nodes" | "pods", ns?: string, enabled = true) {
  const q = useQuery({ ...kubeUsageQuery(env, kind, ns), enabled });
  const index = useMemo(() => new Map((q.data?.rows ?? []).map((r) => [usageKey(r.namespace, r.name), r])), [q.data]);
  const forbidden = q.error instanceof ApiError && q.error.status === 403;
  const available = q.data?.available === true;
  const reason = q.data && !q.data.available ? (q.data.reason ?? "metrics are not available") : forbidden ? "these credentials may not read metrics" : undefined;
  return { q, index, available, reason, rows: available ? (q.data?.rows ?? []) : [] };
}

/** Nodes' usage summed against what they can allocate; `share` is 0–1. */
export function clusterTotals(rows: KubeUsageRow[]) {
  const sized = rows.filter((r) => r.cpu_allocatable && r.memory_allocatable);
  const sum = (f: (r: KubeUsageRow) => number) => sized.reduce((n, r) => n + f(r), 0);
  const cpuAlloc = sum((r) => r.cpu_allocatable ?? 0);
  const memAlloc = sum((r) => r.memory_allocatable ?? 0);
  return {
    cpu: rows.reduce((n, r) => n + r.cpu, 0),
    memory: rows.reduce((n, r) => n + r.memory, 0),
    cpuAlloc,
    memAlloc,
    cpuShare: cpuAlloc ? sum((r) => r.cpu) / cpuAlloc : undefined,
    memShare: memAlloc ? sum((r) => r.memory) / memAlloc : undefined,
  };
}

/** A node's share of one resource, 0–1, when its allocatable is known. */
export const share = (used: number, alloc: number | null | undefined) => (alloc ? used / alloc : undefined);

/**
 * The last `n` readings of a polled value, one per fetch (`at` is the
 * fetch time), so a readout can draw where it has been this visit.
 */
export function useSeries(value: number | undefined, at: number, n = 20): number[] {
  const [s, setS] = useState<{ at: number; values: number[] }>({ at: 0, values: [] });
  if (value !== undefined && at !== s.at) {
    const next = { at, values: [...s.values, value].slice(-n) };
    setS(next);
    return next.values;
  }
  return s.values;
}
