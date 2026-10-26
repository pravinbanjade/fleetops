import { useEffect, useRef } from "react";
import type { ContainerSummary } from "@/api/gen/types.gen";
import { needsAttention, shownState } from "@/lib/health";
import { cn } from "@/lib/utils";

export type Tone = "run" | "wait" | "fail" | "idle";

// A state is a small dot in its hue and the word beside it, never a filled
// tag: a column of them stays quiet, so the failing one is what the eye finds.
const WORD: Record<Tone, string> = {
  run: "text-foreground",
  wait: "text-state-wait-text",
  fail: "text-state-fail-text",
  idle: "text-muted-foreground",
};
const DOT: Record<Tone, string> = {
  run: "bg-state-run",
  wait: "bg-state-wait",
  fail: "bg-state-fail",
  idle: "bg-meta",
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

/** The dot swells and settles: the state changed, without a flash behind the word. */
const PULSE: Keyframe[] = [{ transform: "scale(2.2)" }, { transform: "scale(1)" }];

/**
 * A container's (or compose project's) state as a strip holder: a squared
 * tag tinted in the state's colour, the word exactly as Docker or
 * Kubernetes reports it, in its own case. When
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
    const el = ref.current?.firstElementChild;
    if (!el?.animate || window.matchMedia?.("(prefers-reduced-motion: reduce)").matches) return;
    el.animate(PULSE, { duration: 700, easing: "cubic-bezier(0.16, 1, 0.3, 1)" });
  }, [state, track]);
  return (
    <span
      ref={ref}
      data-slot="state"
      data-tone={t}
      className={cn(
        "inline-flex h-5 shrink-0 items-center gap-1.5 text-[13px] leading-none font-medium whitespace-nowrap",
        WORD[t],
        className,
      )}
    >
      <span aria-hidden className={cn("size-[7px] shrink-0 rounded-full", DOT[t])} />
      {state}
    </span>
  );
}
