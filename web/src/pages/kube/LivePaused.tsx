import { RotateCw } from "lucide-react";
import { Button } from "@/components/ui/button";
import type { WatchPause } from "@/live/kubeWatch";

/** Says a Kubernetes page is no longer live, and since when its data is from. */
export function LivePaused({ paused, onRetry }: { paused: WatchPause; onRetry: () => void }) {
  const time = new Date(paused.at).toLocaleTimeString([], { hour12: false });
  return (
    <div role="status" className="mb-3 flex flex-wrap items-center gap-3 rounded-md bg-state-wait/10 px-3 py-2 text-sm">
      <span>
        Live updates paused — showing data from {time}. <span className="text-muted-foreground">{paused.message}</span>
      </span>
      <Button size="sm" variant="outline" onClick={onRetry}>
        <RotateCw aria-hidden />
        Retry
      </Button>
    </div>
  );
}
