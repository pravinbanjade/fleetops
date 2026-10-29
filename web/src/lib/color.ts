/**
 * A colour faded to `a` (0-1): a #rrggbb gets an alpha byte, an oklch() an
 * alpha term. Anything else comes back as it is, opaque.
 */
export function withAlpha(c: string, a: number): string {
  if (/^#[0-9a-f]{6}$/i.test(c)) return c + Math.round(a * 255).toString(16).padStart(2, "0");
  if (c.startsWith("oklch(")) return c.replace(/\)$/, ` / ${a})`);
  return c;
}
