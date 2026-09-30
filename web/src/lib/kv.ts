/**
 * `key=value` lines (blank lines skipped) into a map, or throws naming the
 * bad line: `Label "x" must look like key=value`.
 */
export function parseKeyValues(text: string, what: string): Record<string, string> {
  const out: Record<string, string> = {};
  for (const raw of text.split("\n")) {
    const l = raw.trim();
    if (!l) continue;
    const i = l.indexOf("=");
    if (i <= 0) throw new Error(`${what} "${l}" must look like key=value`);
    out[l.slice(0, i)] = l.slice(i + 1);
  }
  return out;
}

/** The inverse of `parseKeyValues`. */
export function formatKeyValues(map: Record<string, string> | null | undefined): string {
  return Object.entries(map ?? {})
    .map(([k, v]) => `${k}=${v}`)
    .join("\n");
}
