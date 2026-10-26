/** Points drawn: a minute at one sample a second. */
const POINTS = 60;
const W = 80;
const H = 20;

/**
 * A small line of recent values, newest on the right. `max` sets the top
 * of the scale; values above it (and gaps, as null) are drawn clipped or
 * skipped rather than rescaling the line. `window` is how many readings
 * the full width holds (60 for one a second). `fluid` stretches the line to
 * its box's width, so a narrow card can't push it past its edge. Below
 * `minPoints` readings only the baseline is drawn: on a slow series two
 * points read as a stray tick.
 */
export function Sparkline({
  values,
  max,
  label,
  width = W,
  height = H,
  window = POINTS,
  fluid = false,
  minPoints = 2,
}: {
  values: (number | null)[];
  max: number;
  label: string;
  width?: number;
  height?: number;
  window?: number;
  fluid?: boolean;
  minPoints?: number;
}) {
  const recent = values.slice(-window);
  const step = width / (window - 1);
  const offset = window - recent.length;
  const points = recent
    .map((v, i) => (v === null ? null : `${((offset + i) * step).toFixed(1)},${(height - (Math.min(v, max) / max) * (height - 2) - 1).toFixed(1)}`))
    .filter((p) => p !== null)
    .join(" ");
  const drawn = recent.filter((v) => v !== null).length >= minPoints;
  return (
    <svg
      role="img"
      aria-label={label}
      width={fluid ? "100%" : width}
      height={height}
      viewBox={`0 0 ${width} ${height}`}
      preserveAspectRatio="none"
      className="block text-meta"
    >
      {/* The minute's full window, so a short history reads as recent, not as a stub. */}
      <line x1={0} x2={width} y1={height - 1} y2={height - 1} stroke="currentColor" strokeOpacity={0.18} strokeWidth={1} vectorEffect="non-scaling-stroke" />
      {drawn && points && (
        <polyline points={points} fill="none" stroke="currentColor" strokeWidth={1.25} strokeLinejoin="round" vectorEffect="non-scaling-stroke" />
      )}
    </svg>
  );
}
