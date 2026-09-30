import { describe, expect, it } from "vitest";
import { summarizeDiskUsage } from "./df";

describe("summarizeDiskUsage", () => {
  it("reads the totals newer engines send", () => {
    expect(
      summarizeDiskUsage({
        ImageUsage: { TotalSize: 3000, Reclaimable: 1000 },
        ContainerUsage: { TotalSize: 20, Reclaimable: 5 },
        VolumeUsage: { TotalSize: 400 },
        BuildCacheUsage: { TotalSize: 7, Reclaimable: 7 },
      }),
    ).toEqual({
      images: { size: 3000, reclaimable: 1000 },
      containers: { size: 20, reclaimable: 5 },
      volumes: { size: 400, reclaimable: undefined },
      buildCache: { size: 7, reclaimable: 7 },
    });
  });

  it("sums the item lists older engines send", () => {
    expect(
      summarizeDiskUsage({
        LayersSize: 3000,
        Containers: [{ SizeRw: 5 }, { SizeRw: 15 }, {}],
        Volumes: [{ UsageData: { Size: 400 } }, { UsageData: null }],
        BuildCache: null,
      }),
    ).toEqual({ images: { size: 3000 }, containers: { size: 20 }, volumes: { size: 400 }, buildCache: { size: 0 } });
  });
});
