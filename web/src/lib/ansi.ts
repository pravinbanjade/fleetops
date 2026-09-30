/**
 * ANSI SGR (colour and style) parsing for log lines. Other escape
 * sequences (cursor movement, OSC titles and links) are dropped. Each line
 * is parsed on its own: a colour left open at the end of a line does not
 * carry over to the next.
 */

/** A palette index (0–255) or a `#rrggbb` truecolour. */
export type Color = number | string;

export interface Style {
  fg?: Color;
  bg?: Color;
  bold?: boolean;
  dim?: boolean;
  italic?: boolean;
  underline?: boolean;
  inverse?: boolean;
}

export interface Segment {
  text: string;
  style: Style;
}

// CSI sequences (any final byte) and OSC sequences ended by BEL or ST.
// eslint-disable-next-line no-control-regex
const ESCAPES = /\x1b\[([0-9;:?]*)([@-~])|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b[@-Z\\-_]/g;
// eslint-disable-next-line no-control-regex
const CONTROL = /[\x00-\x08\x0b-\x1f\x7f]/g;

/** The line's text with every escape sequence and control character removed. */
export function stripAnsi(text: string): string {
  if (!text.includes("\x1b")) return text.replace(CONTROL, "");
  return text.replace(ESCAPES, "").replace(CONTROL, "");
}

/** Splits a line into runs of text that share a style. */
export function parseAnsi(text: string): Segment[] {
  const out: Segment[] = [];
  let style: Style = {};
  let last = 0;
  const push = (s: string) => {
    const clean = s.replace(CONTROL, "");
    if (!clean) return;
    const prev = out[out.length - 1];
    if (prev && prev.style === style) prev.text += clean;
    else out.push({ text: clean, style });
  };
  for (const m of text.matchAll(ESCAPES)) {
    push(text.slice(last, m.index));
    last = m.index + m[0].length;
    if (m[2] === "m") style = applySgr(style, m[1]);
  }
  push(text.slice(last));
  return out;
}

function applySgr(prev: Style, params: string): Style {
  const codes = (params === "" ? "0" : params).split(/[;:]/).map((p) => (p === "" ? 0 : Number(p)));
  let s: Style = { ...prev };
  for (let i = 0; i < codes.length; i++) {
    const c = codes[i];
    if (c === 0) s = {};
    else if (c === 1) s.bold = true;
    else if (c === 2) s.dim = true;
    else if (c === 3) s.italic = true;
    else if (c === 4) s.underline = true;
    else if (c === 7) s.inverse = true;
    else if (c === 22) s.bold = s.dim = undefined;
    else if (c === 23) s.italic = undefined;
    else if (c === 24) s.underline = undefined;
    else if (c === 27) s.inverse = undefined;
    else if (c >= 30 && c <= 37) s.fg = c - 30;
    else if (c >= 90 && c <= 97) s.fg = c - 90 + 8;
    else if (c === 39) s.fg = undefined;
    else if (c >= 40 && c <= 47) s.bg = c - 40;
    else if (c >= 100 && c <= 107) s.bg = c - 100 + 8;
    else if (c === 49) s.bg = undefined;
    else if (c === 38 || c === 48) {
      const [color, used] = extended(codes, i + 1);
      if (color !== undefined) s[c === 38 ? "fg" : "bg"] = color;
      i += used;
    }
  }
  return s;
}

/** `5;n` or `2;r;g;b` after a 38/48; returns the colour and how many codes it used. */
function extended(codes: number[], i: number): [Color | undefined, number] {
  if (codes[i] === 5) {
    const n = codes[i + 1];
    return [n >= 0 && n <= 255 ? n : undefined, 2];
  }
  if (codes[i] === 2) {
    const rgb = codes.slice(i + 1, i + 4);
    if (rgb.length < 3 || rgb.some((v) => !(v >= 0 && v <= 255))) return [undefined, 4];
    return ["#" + rgb.map((v) => v.toString(16).padStart(2, "0")).join(""), 4];
  }
  return [undefined, 0];
}

/**
 * A CSS colour for a palette index or truecolour. The 16 basic colours
 * come from the theme (`--ansi-0` … `--ansi-15` in styles.css) so they
 * read in light and dark mode; 16–255 are the xterm cube and grey ramp.
 */
export function cssColor(c: Color): string {
  if (typeof c === "string") return c;
  if (c < 16) return `var(--ansi-${c})`;
  if (c >= 232) {
    const v = 8 + (c - 232) * 10;
    return `rgb(${v}, ${v}, ${v})`;
  }
  const n = c - 16;
  const level = (x: number) => (x === 0 ? 0 : 55 + x * 40);
  return `rgb(${level(Math.floor(n / 36))}, ${level(Math.floor(n / 6) % 6)}, ${level(n % 6)})`;
}
