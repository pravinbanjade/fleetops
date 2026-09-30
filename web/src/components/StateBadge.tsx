import { useEffect, useRef } from "react";
import type { ContainerSummary } from "@/api/gen/types.gen";
import { needsAttention, shownState } from "@/lib/health";
import { cn } from "@/lib/utils";

export type Tone = "run" | "wait" | "fail" | "idle";

// Holder fills and inks are theme variables chosen for at least 4.5:1
// contrast (WCAG AA) in both themes.
const FILL: Record<Tone, string> = {
  run: "bg-state-run text-state-run-ink",
  wait: "bg-state-wait text-state-wait-ink",
  fail: "bg-state-fail text-state-fail-ink",
  idle: "border border-input text-muted-foreground",
};

const TONE: Record<string, Tone> = {
  running: "run",
  healthy: "run",
  up: "run",
  paused: "wait",
  restarting: "wait",
  partial: "wait",
  unhealthy: "fail",
  dead: "fail",
  down: "fail",
};

/** The tone a state reads as: the holder colour of its strip. */
export function stateTone(state: string): Tone {
  return TONE[state] ?? "idle";
}

/**
 * A container's holder: its shown state, in red when it stopped against its
 * will (a non-zero exit), neutral when it exited cleanly.
 */
export function ContainerState({ container, className }: { container: ContainerSummary; className?: string }) {
  const state = shownState(container);
  return (
    <StateBadge
      state={state}
      tone={state === "exited" && needsAttention(container) ? "fail" : undefined}
      className={className}
    />
  );
}

const FLASH: Keyframe[] = [
  { boxShadow: "0 0 0 0 oklch(0.82 0.15 78 / 0.75)" },
  { boxShadow: "0 0 0 7px oklch(0.82 0.15 78 / 0)" },
];

/**
 * A container's (or compose project's) state as a strip holder: a squared
 * tag in the state's colour, the word exactly as Docker reports it. When
 * Docker changes the state on screen, the holder flashes once.
 */
export function StateBadge({ state, tone, className }: { state: string; tone?: Tone; className?: string }) {
  const t = tone ?? stateTone(state);
  const ref = useRef<HTMLSpanElement>(null);
  const seen = useRef(state);
  useEffect(() => {
    if (seen.current === state) return;
    seen.current = state;
    const el = ref.current;
    if (!el?.animate || window.matchMedia?.("(prefers-reduced-motion: reduce)").matches) return;
    el.animate(FLASH, { duration: 900, easing: "cubic-bezier(0.16, 1, 0.3, 1)" });
  }, [state]);
  return (
    <span
      ref={ref}
      data-slot="state"
      data-tone={t}
      className={cn(
        "font-strip inline-flex h-5 min-w-[4.75rem] shrink-0 items-center justify-center rounded-xs px-1.5 text-[11px] leading-none font-semibold tracking-[0.04em] whitespace-nowrap uppercase",
        FILL[t],
        className,
      )}
    >
      {state}
    </span>
  );
}
