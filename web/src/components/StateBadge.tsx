import { Badge } from "@/components/ui/badge";
import { cn } from "@/lib/utils";

const TONE: Record<string, string> = {
  running: "bg-emerald-600 text-white",
  healthy: "bg-emerald-600 text-white",
  paused: "bg-amber-500 text-white",
  restarting: "bg-amber-500 text-white",
  partial: "bg-amber-500 text-white",
  unhealthy: "bg-destructive text-white",
  dead: "bg-destructive text-white",
};

/** A container's (or compose project's) state as a colored badge. */
export function StateBadge({ state }: { state: string }) {
  return (
    <Badge variant={TONE[state] ? "default" : "outline"} className={cn(TONE[state])}>
      {state}
    </Badge>
  );
}
