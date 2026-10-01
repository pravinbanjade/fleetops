import type { RowHealth } from "@/api/gen/types.gen";
import { cn } from "@/lib/utils";

const TONE: Record<RowHealth, string> = { ok: "bg-live", warn: "bg-state-wait", bad: "bg-state-fail", unknown: "bg-muted-foreground/50" };

export function KubeHealth({ health, label }: { health: RowHealth; label: string }) {
  return (
    <span className="flex items-center gap-2">
      <span aria-hidden className={cn("size-2 shrink-0 rounded-full", TONE[health])} />
      {label}
    </span>
  );
}
