/**
 * Splits a command line into arguments the way a shell would for simple
 * quoting: whitespace separates, and single or double quotes group
 * (`nginx -g "daemon off;"` is three arguments). A backslash escapes the
 * next character outside single quotes.
 */
export function splitArgs(line: string): string[] {
  const out: string[] = [];
  let cur = "";
  let inWord = false;
  let quote: '"' | "'" | null = null;
  for (let i = 0; i < line.length; i++) {
    const ch = line[i];
    if (quote) {
      if (ch === quote) quote = null;
      else if (ch === "\\" && quote === '"' && i + 1 < line.length) cur += line[++i];
      else cur += ch;
    } else if (ch === '"' || ch === "'") {
      quote = ch;
      inWord = true;
    } else if (ch === "\\" && i + 1 < line.length) {
      cur += line[++i];
      inWord = true;
    } else if (/\s/.test(ch)) {
      if (inWord) out.push(cur);
      cur = "";
      inWord = false;
    } else {
      cur += ch;
      inWord = true;
    }
  }
  if (quote) throw new Error(`unclosed ${quote} quote`);
  if (inWord) out.push(cur);
  return out;
}

/** The inverse of `splitArgs`: quotes arguments that need it. */
export function joinArgs(args: string[]): string {
  return args
    .map((a) => (a === "" ? '""' : /[\s"'\\]/.test(a) ? `"${a.replace(/(["\\])/g, "\\$1")}"` : a))
    .join(" ");
}
