/** Points drawn: a minute at one sample a second. */
const POINTS = 60;
const W = 80;
const H = 20;

/**
 * A small line of recent values, newest on the right. `max` sets the top
 * of the scale; values above it (and gaps, as null) are drawn clipped or
 * skipped rather than rescaling the line.
 */
export function Sparkline({ values, max, label }: { values: (number | null)[]; max: number; label: string }) {
  const recent = values.slice(-POINTS);
  const step = W / (POINTS - 1);
  const offset = POINTS - recent.length;
  const points = recent
    .map((v, i) => (v === null ? null : `${((offset + i) * step).toFixed(1)},${(H - (Math.min(v, max) / max) * (H - 2) - 1).toFixed(1)}`))
    .filter((p) => p !== null)
    .join(" ");
  return (
    <svg role="img" aria-label={label} width={W} height={H} viewBox={`0 0 ${W} ${H}`} className="text-primary/70">
      {points && <polyline points={points} fill="none" stroke="currentColor" strokeWidth={1.25} strokeLinejoin="round" />}
    </svg>
  );
}
