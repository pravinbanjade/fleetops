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
      track={container.id}
      tone={state === "exited" && needsAttention(container) ? "fail" : undefined}
      className={className}
    />
  );
}

/** The last state each tracked holder showed. */
const LAST = new Map<string, string>();

/** A ring of the cord colour, in the current theme, spreading out and fading. */
function flash(el: HTMLElement): Keyframe[] {
  const cord = getComputedStyle(el).getPropertyValue("--primary").trim() || "oklch(0.82 0.15 78)";
  const fade = (a: number) => (cord.startsWith("oklch(") ? cord.replace(/\)$/, ` / ${a})`) : cord);
  return [{ boxShadow: `0 0 0 0 ${fade(0.75)}` }, { boxShadow: `0 0 0 7px ${fade(0)}` }];
}

/**
 * A container's (or compose project's) state as a strip holder: a squared
 * tag in the state's colour, the word exactly as Docker reports it. When
 * Docker changes the state on screen, the holder flashes once.
 */
export function StateBadge({
  state,
  tone,
  track,
  className,
}: {
  state: string;
  tone?: Tone;
  /** An ID to remember the state under, so a strip that moves bays (and remounts) still flashes. */
  track?: string;
  className?: string;
}) {
  const t = tone ?? stateTone(state);
  const ref = useRef<HTMLSpanElement>(null);
  const seen = useRef(state);
  useEffect(() => {
    const before = track ? LAST.get(track) : seen.current;
    seen.current = state;
    if (track) LAST.set(track, state);
    if (before === undefined || before === state) return;
    const el = ref.current;
    if (!el?.animate || window.matchMedia?.("(prefers-reduced-motion: reduce)").matches) return;
    el.animate(flash(el), { duration: 900, easing: "cubic-bezier(0.16, 1, 0.3, 1)" });
  }, [state, track]);
  return (
    <span
      ref={ref}
      data-slot="state"
      data-tone={t}
      className={cn(
        "font-holder inline-flex h-5 min-w-[4.75rem] shrink-0 items-center justify-center rounded-xs px-1.5 text-[11px] leading-none font-semibold tracking-[0.04em] whitespace-nowrap uppercase",
        FILL[t],
        className,
      )}
    >
      {state}
    </span>
  );
}
