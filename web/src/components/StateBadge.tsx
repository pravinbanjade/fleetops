import { Badge } from "@/components/ui/badge";
import { cn } from "@/lib/utils";

// Fills and text chosen for at least 4.5:1 contrast (WCAG AA) in both
// themes: white on the darker green and red, near-black on amber.
const RUNNING = "bg-emerald-700 text-white";
const CAUTION = "bg-amber-500 text-amber-950";
const FAILED = "bg-red-700 text-white";

const TONE: Record<string, string> = {
  running: RUNNING,
  healthy: RUNNING,
  up: RUNNING,
  paused: CAUTION,
  restarting: CAUTION,
  partial: CAUTION,
  unhealthy: FAILED,
  dead: FAILED,
  down: FAILED,
};

/** A container's (or compose project's) state as a colored badge. */
export function StateBadge({ state }: { state: string }) {
  return (
    <Badge variant={TONE[state] ? "default" : "outline"} className={cn(TONE[state])}>
      {state}
    </Badge>
  );
}
