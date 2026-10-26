import { memo, useEffect, useState } from "react";
import { diffDigits } from "@/lib/tick";
import { cn } from "@/lib/utils";

type Lit = { text: string; value: number; marks: boolean[]; dir: "rise" | "fall" | null };

/**
 * A live figure. When its text changes, only the characters that changed
 * light up, in the rise or fall colour, and fade back to the ink: the eye
 * goes to what moved and nothing else blinks. Never a background.
 */
export const Tick = memo(function Tick({ value, text, className }: { value: number; text: string; className?: string }) {
  const [lit, setLit] = useState<Lit>({ text, value, marks: [], dir: null });
  if (lit.text !== text) {
    const dir = value > lit.value ? "rise" : value < lit.value ? "fall" : null;
    setLit({ text, value, marks: dir ? diffDigits(lit.text, text) : [], dir });
  }
  useEffect(() => {
    if (!lit.dir) return;
    // Two frames: the lit colour paints first, then its removal transitions back.
    let second = 0;
    const first = requestAnimationFrame(() => {
      second = requestAnimationFrame(() => setLit((l) => (l.text === lit.text ? { ...l, marks: [], dir: null } : l)));
    });
    return () => {
      cancelAnimationFrame(first);
      cancelAnimationFrame(second);
    };
  }, [lit.text, lit.dir]);
  const n = text.length;
  return (
    <span className={cn("font-figure", className)}>
      {/* Read as one value; the per-character spans are only for the eye. */}
      <span className="sr-only">{text}</span>
      {[...text].map((c, i) => (
        // Keyed from the right, so a digit keeps its node (and its fade) as the figure grows.
        <span key={n - i} aria-hidden className={cn("transition-colors duration-900 ease-out", lit.dir && lit.marks[i] && `tick-${lit.dir}`)}>
          {c}
        </span>
      ))}
    </span>
  );
});
