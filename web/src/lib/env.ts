/** Hues an environment's identity tag can take: none of them a state colour. */
const HUES = [255, 292, 328, 205, 232, 270];

/** A stable hue for an environment, so prod and staging never look alike. */
export function envHue(name: string): number {
  let h = 0;
  for (const ch of name) h = (h * 31 + ch.charCodeAt(0)) >>> 0;
  return HUES[h % HUES.length];
}

/** Up to two letters naming an environment on its tag. */
export function envInitials(name: string): string {
  const parts = name.split(/[^a-zA-Z0-9]+/).filter(Boolean);
  const s = parts.length > 1 ? parts[0][0] + parts[1][0] : name.slice(0, 2);
  return s.toUpperCase();
}
