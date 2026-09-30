import type { DiskUsage, UsageTotals } from "@/api/queries";

export interface Usage {
  size: number;
  /** Absent on engines that do not report it. */
  reclaimable?: number;
}

export interface DiskSummary {
  images: Usage;
  containers: Usage;
  volumes: Usage;
  buildCache: Usage;
}

const sum = (xs: (number | undefined)[] | null | undefined) => (xs ?? []).reduce<number>((a, b) => a + (b ?? 0), 0);

const totals = (u: UsageTotals): Usage => ({ size: u.TotalSize ?? 0, reclaimable: u.Reclaimable });

/** Sizes per resource kind from either shape of Docker's `system df`. */
export function summarizeDiskUsage(df: DiskUsage): DiskSummary {
  if (df.ImageUsage || df.ContainerUsage || df.VolumeUsage || df.BuildCacheUsage) {
    return {
      images: totals(df.ImageUsage ?? {}),
      containers: totals(df.ContainerUsage ?? {}),
      volumes: totals(df.VolumeUsage ?? {}),
      buildCache: totals(df.BuildCacheUsage ?? {}),
    };
  }
  return {
    images: { size: df.LayersSize ?? sum(df.Images?.map((i) => i.Size)) },
    containers: { size: sum(df.Containers?.map((c) => c.SizeRw)) },
    volumes: { size: sum(df.Volumes?.map((v) => v.UsageData?.Size)) },
    buildCache: { size: sum(df.BuildCache?.map((b) => b.Size)) },
  };
}
