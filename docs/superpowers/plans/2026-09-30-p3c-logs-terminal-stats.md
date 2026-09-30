# FleetOps P3c — Logs, Terminal and Stats Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the container page its live tabs and the containers list its live usage columns:
- a **Logs** tab: virtualised, 200k-line buffer, follow with auto-pause, search, stream/timestamp/wrap/tail/since controls, ANSI colours, and download;
- a **Terminal** tab: xterm.js over the websocket `exec` topic, with resize, shell choice and a user override;
- a **Stats** tab with live CPU, memory, network and block I/O charts;
- **CPU and memory sparklines** in the containers list, for the rows on screen.

**Architecture:**
- **No server changes.** P2b's websocket already serves the `logs`, `exec` and `stats` topics (spec §5.2). This plan is web-only, and `src/api/gen` does not change.
- **Logs:**
  - `useLogStream` follows a container's log into a `LogBuffer`, a ring of at most 200k entries with increasing `seq` numbers.
  - Timestamps are always requested. The viewer shows or hides them without asking the server again. A subscription that replays its tail can also resume after the last timestamp held, so a websocket reconnect or a container restart adds only new lines.
  - `LogView` renders only the rows in view (TanStack Virtual).
  - Search (`LogMatches`) scans only lines it has not seen.
- **Terminal:**
  - `TerminalView` owns one xterm.js terminal and one `exec` subscription. Output arrives as binary frames, keystrokes go back as binary frames, and resizes go out as `resize` frames.
  - The subscription is made with the new `WsClient` option `resubscribe: false`. On a dropped connection it ends as `lost` instead of silently starting a new process on reconnect.
- **Stats:**
  - A tab-wide `StatsStore` (in `LiveProvider`) holds one `stats` subscription per container, however many components read it. It keeps about two minutes of history, with byte counters turned into rates.
  - A reader that goes away keeps the subscription alive for 5 s. Moving from the list to a container page therefore keeps the history.
  - The list's CPU and memory cells subscribe only while their row is on screen (`useInView`) and the container is running.
- **Bundle:** xterm.js (~340 kB) and uPlot load only when their tab opens (`React.lazy`). TanStack Virtual lives in the container page's chunk.

**Tech Stack:** P3b's stack, plus:
- `@tanstack/react-virtual` 3.14;
- `@xterm/xterm` 6.0 and `@xterm/addon-fit` 0.11;
- `uplot` 1.6.

**Spec:** `docs/superpowers/specs/2026-09-29-fleetops-core-docker-design.md`:
- §4.2: the container detail tabs Stats, Logs and Terminal, and the list's live CPU/mem sparklines.
- §4.3: the log viewer, the terminal, and stats for visible containers only.
- §5.2: the websocket topics, `gap`, binary exec frames and resubscribe on reconnect.
- §7: exec is audited (server side), which is why the terminal waits for Connect.

**Plan series for sub-project 1:**
- P1, P2a–P2c, P3a and P3b are done.
- **P3c (this plan):** logs, terminal and stats.
- P3d: Playwright e2e, `make image` and CI.

## Global Constraints

- **Everything from P3a's and P3b's Global Constraints still holds:**
  - Node ≥24 and pnpm 12.8.1; TypeScript `~6.0.3`.
  - The generated client is never edited by hand, and `pnpm check:api` stays clean.
  - Query keys start with `["env", env]`.
  - Errors show inline (`role="alert"`) or as a toast.
  - Pages load lazily.
  - `src/test/setup.ts`'s 3 s `asyncUtilTimeout`.
- **Container tabs:**
  - The order is `overview, inspect, env, mounts, networks, stats, logs, terminal, files`, in `pages/containers/tabs.ts`.
  - A tab's content mounts only while the tab is open (Radix `TabsContent`), so its subscriptions last only as long as the tab.
- **Colours:**
  - The CSP allows inline styles (`style-src 'self' 'unsafe-inline'`), so ANSI colours are inline `style` attributes.
  - The 16 basic ANSI colours are theme variables, `--ansi-0` … `--ansi-15` in `styles.css`.
  - Canvas-drawn colours (uPlot, xterm.js) are concrete hex values, because a canvas cannot read CSS variables.
- **Tests:**
  - jsdom has no layout. `LogView` treats a 0-height measurement as a 1000×600 viewport, so rows render in tests.
  - jsdom does not scroll, so tests fire `scroll` events after setting `scrollTop`, `scrollHeight` and `clientHeight` by hand.
  - xterm.js and uPlot are replaced with `vi.mock` stand-ins that record what they were given. jsdom has no canvas.
  - Binary frames in tests are `ArrayBuffer`s passed to `FakeWebSocket.receive`.
- **Spec deltas (intentional):**
  - **The list's sparklines are inline SVG** (`Sparkline`), not uPlot. There are two per running row, so a canvas chart each would cost more than it shows. uPlot draws the Stats tab's charts.
  - **Which query changes start the log over:**
    - The stdout/stderr toggles and the tail and time-range controls re-request from the server with a new buffer. They are query changes, and Docker applies `tail` per request.
    - Timestamps and wrapping are display-only.
  - **ANSI state is per line.** A colour left open at the end of a line does not carry over to the next.
  - **Search is case-insensitive** for both text and regex. There is no case toggle.
  - **"Download full log"** uses a one-shot `logs` subscription (`follow: false, tail: null`) instead of a new REST endpoint. "Download shown" saves the buffer.
  - **The terminal starts nothing until Connect.** Every exec is audited, and opening the tab should not create an entry.
  - **The terminal is always dark.**
  - **Copy is Ctrl+Shift+C**, because Ctrl+C belongs to the process. Paste is the browser's.
  - **Stats history is 120 samples**, about two minutes. The list draws the last 60.
  - **`DataTable`'s root grid becomes `minmax(0,1fr)`.** The two new columns made the containers table wider than the page, and the grid item was widening the page instead of letting the table scroll in its box. Found in the smoke run; there is no jsdom test, since jsdom has no layout.

## Review Focus

1. **The websocket reconnects, or the container restarts, while the log is open.** The log must carry on without repeating lines it already shows, including when Docker trims timestamp fractions to different lengths. Lines the user cleared must stay cleared. Pinned in:
   - Task 1: `after resume, skips replayed lines and keeps what is new`, `resumes correctly when the fraction lengths differ`, and `clears lines but still skips them on a later resume`.
   - Task 2: `resumes after a reconnect without repeating lines`, `says when the container is stopped, and picks up when it starts again`, and `clears the lines shown and keeps them cleared after a reconnect`.
2. **A huge or fast log.** For example, 250k lines at once, or the browser falling behind the server. The page must stay responsive, keep the newest 200k, render only the rows in view, and say where lines were skipped. Pinned in:
   - Task 1: `drops the oldest lines past its limit and keeps seqs increasing`.
   - Task 2: `keeps a 250k-line log to the newest 200k and renders only the rows in view`, and `marks lines the server skipped`.
3. **The connection drops during a terminal session.** No new shell may start behind the user's back on reconnect, which would be an unasked-for, audited exec. The user must be told the session ended. Pinned in:
   - Task 4: `ends a subscription made without resubscribe when the connection drops`, and `ends the session when the connection drops, and does not start another on reconnect`.
4. **A search the user types that is not a valid pattern, or that matches the empty string.** Examples: `(`, or `x*`. There must be no crash and no infinite loop; the user sees "Invalid regular expression". Pinned in:
   - Task 1: `does not loop on a pattern that matches the empty string`, and `supports regular expressions and reports a bad one`.
   - Task 3: `searches with a regular expression and says when it is invalid`.
5. **A containers list with many rows.** Stats must be subscribed only for running containers whose row is on screen, once per container even though two cells read it. Pinned in:
   - Task 5: `shows live CPU and memory for running containers, one subscription each`, `subscribes only for rows on screen`, and `shares one subscription between readers and keeps a bounded history`.

---

## File Structure

```
web/
  package.json, pnpm-lock.yaml             + @tanstack/react-virtual, @xterm/xterm, @xterm/addon-fit, uplot
  src/
    styles.css                             + --ansi-0 … --ansi-15, light and dark
    lib/
      ansi.ts                              parseAnsi, stripAnsi, cssColor
      logs.ts                              LogBuffer, LogMatches, compileQuery, matchRanges, markSegments, logText, timestamps
      download.ts                          saveText
      format.ts                            (modify) + formatPercent
    live/
      ws.ts                                (modify) subscribe(…, { resubscribe: false }) and onLost
      stats.ts                             StatsStore, toPoint, STATS_HISTORY
      LiveProvider.tsx                     (modify) the tab's StatsStore; useContainerStats
    components/
      Sparkline.tsx                        inline SVG line
      Chart.tsx                            uPlot wrapper
      useInView.ts                         IntersectionObserver hook
      DataTable.tsx                        (modify) root grid minmax(0,1fr)
    pages/containers/
      tabs.ts, ContainerPage.tsx           (modify) Stats, Logs and Terminal tabs
      ContainersPage.tsx                   (modify) CPU and Memory columns
      logs/{useLogStream.ts,LogView.tsx,LogsTab.tsx}
      terminal/{TerminalView.tsx,TerminalTab.tsx}
      stats/StatsTab.tsx
README.md                                  status and feature list
```

**Test commands.**
- `cd web && pnpm test <paths>` runs the named tests.
- The whole-suite gate is `pnpm test && pnpm typecheck && pnpm lint` from `web/`, or `make test-web` from the repo root, which also runs `check:api`.

---

### Task 1: ANSI parsing, the log buffer and search

**Files:**
- Create: `web/src/lib/ansi.ts` and `web/src/lib/logs.ts`.
- Modify: `web/package.json` and `pnpm-lock.yaml` (via `pnpm add`), and `web/src/styles.css`.
- Test: `web/src/lib/ansi.test.ts` and `web/src/lib/logs.test.ts`.

**Interfaces:**
- Consumes: `LogLine` from `@/api/gen/types.gen` (`{ stream: "stdout" | "stderr"; text: string }`).
- Produces `@/lib/ansi`:
  - `type Color = number | string` (a palette index 0–255 or `#rrggbb`);
  - `interface Style { fg?, bg?, bold?, dim?, italic?, underline?, inverse? }`;
  - `interface Segment { text: string; style: Style }`;
  - `parseAnsi(text): Segment[]`, `stripAnsi(text): string` and `cssColor(c: Color): string`.
- Produces `@/lib/logs`:
  - `LOG_BUFFER_MAX = 200_000`.
  - `interface LogEntry { seq; stream: "stdout" | "stderr" | "gap"; ts: string | null; text; dropped? }`.
  - `splitTimestamp(text): [ts | null, rest]` and `timestampKey(ts): string`.
  - `class LogBuffer(max?)`:
    - `length`, `firstSeq`, `version`;
    - `at(i)`, `bySeq(seq)`, `entries()`;
    - `append(lines: LogLine[])`, `gap(dropped)`;
    - `resume()`, which skips replayed lines on the next append;
    - `clear()`, which drops the lines but keeps the resume position;
    - `reset()`, which drops everything.
  - `compileQuery(query, regex): RegExp | { error } | null`, which is case-insensitive and global.
  - `matchRanges(text, re): [start, end][]`.
  - `class LogMatches(buffer)`, with `seqs: number[]`, `setQuery(re)` and `update()`. Task 3 adds `sync(re)`.
  - `logText(entries, timestamps): string`.

- [ ] **Step 1: Add the dependencies for this plan**

Run: `cd web && pnpm add @tanstack/react-virtual@^3.14.13 @xterm/xterm@^6.0.0 @xterm/addon-fit@^0.11.0 uplot@^1.6.32`
Expected: four `+` lines: `@tanstack/react-virtual 3.14.13`, `@xterm/xterm 6.0.0`, `@xterm/addon-fit 0.11.0` and `uplot 1.6.32`. `package.json` and `pnpm-lock.yaml` change.

- [ ] **Step 2: Write the failing tests**

`web/src/lib/ansi.test.ts`:

```ts
import { cssColor, parseAnsi, stripAnsi } from "./ansi";

describe("parseAnsi", () => {
  it("returns one plain segment for text without escapes", () => {
    expect(parseAnsi("hello world")).toEqual([{ text: "hello world", style: {} }]);
  });

  it("splits text at colour changes and resets", () => {
    expect(parseAnsi("a \x1b[31mred\x1b[0m b")).toEqual([
      { text: "a ", style: {} },
      { text: "red", style: { fg: 1 } },
      { text: " b", style: {} },
    ]);
  });

  it("reads bold, bright, background, 256-colour and truecolour codes", () => {
    const [bold, bright, bg, c256, rgb] = parseAnsi(
      "\x1b[1mB\x1b[0;92mG\x1b[0;44mU\x1b[0;38;5;208mO\x1b[0;38;2;255;0;128mP",
    );
    expect(bold.style).toEqual({ bold: true });
    expect(bright.style).toEqual({ fg: 10 });
    expect(bg.style).toEqual({ bg: 4 });
    expect(c256.style).toEqual({ fg: 208 });
    expect(rgb.style).toEqual({ fg: "#ff0080" });
  });

  it("keeps styles that later codes do not change", () => {
    expect(parseAnsi("\x1b[1;31mx\x1b[39my")).toEqual([
      { text: "x", style: { bold: true, fg: 1 } },
      { text: "y", style: { bold: true } },
    ]);
  });

  it("treats an empty parameter list as a reset", () => {
    expect(parseAnsi("\x1b[31mx\x1b[my")[1]).toEqual({ text: "y", style: {} });
  });

  it("drops cursor movement, OSC sequences and control characters", () => {
    expect(parseAnsi("\x1b[2K\x1b]0;title\x07a\x07b\x1b[1Gc")).toEqual([{ text: "abc", style: {} }]);
  });

  it("ignores a truncated extended colour", () => {
    expect(parseAnsi("\x1b[38;2;10mx")).toEqual([{ text: "x", style: {} }]);
  });
});

describe("stripAnsi", () => {
  it("returns the text a reader sees", () => {
    expect(stripAnsi("\x1b[1;32mINFO\x1b[0m started")).toBe("INFO started");
    expect(stripAnsi("plain\ttab")).toBe("plain\ttab");
  });
});

describe("cssColor", () => {
  it("uses theme variables for the 16 basic colours and computes the rest", () => {
    expect(cssColor(1)).toBe("var(--ansi-1)");
    expect(cssColor(196)).toBe("rgb(255, 0, 0)");
    expect(cssColor(232)).toBe("rgb(8, 8, 8)");
    expect(cssColor("#123456")).toBe("#123456");
  });
});
```

`web/src/lib/logs.test.ts`:

```ts
import type { LogLine } from "@/api/gen/types.gen";
import { compileQuery, LogBuffer, LogMatches, logText, matchRanges, splitTimestamp, timestampKey } from "./logs";

const out = (text: string): LogLine => ({ stream: "stdout", text });
const at = (sec: number, text: string, frac = "") =>
  out(`2026-09-30T10:00:${String(sec).padStart(2, "0")}${frac}Z ${text}`);
const texts = (b: LogBuffer) => [...b.entries()].map((e) => (e.stream === "gap" ? `gap ${e.dropped}` : e.text));

describe("splitTimestamp", () => {
  it("separates Docker's timestamp from the line", () => {
    expect(splitTimestamp("2026-09-30T10:00:00.123456789Z hello there")).toEqual([
      "2026-09-30T10:00:00.123456789Z",
      "hello there",
    ]);
    expect(splitTimestamp("no timestamp here")).toEqual([null, "no timestamp here"]);
  });
});

describe("timestampKey", () => {
  it("orders timestamps whose fractions Docker trimmed to different lengths", () => {
    const a = timestampKey("2026-09-30T10:00:00.5Z");
    const b = timestampKey("2026-09-30T10:00:00.123456789Z");
    const c = timestampKey("2026-09-30T10:00:00Z");
    expect(c < b && b < a).toBe(true);
  });

  it("converts an offset to UTC", () => {
    expect(timestampKey("2026-09-30T12:00:00.250+02:00")).toBe(timestampKey("2026-09-30T10:00:00.25Z"));
  });
});

describe("LogBuffer", () => {
  it("drops the oldest lines past its limit and keeps seqs increasing", () => {
    const b = new LogBuffer(3);
    b.append(["a", "b", "c", "d", "e"].map(out));
    expect(texts(b)).toEqual(["c", "d", "e"]);
    expect(b.firstSeq).toBe(2);
    expect(b.bySeq(4)?.text).toBe("e");
    expect(b.at(3)).toBeUndefined();
  });

  it("records a gap where the server skipped lines", () => {
    const b = new LogBuffer();
    b.append([out("a")]);
    b.gap(40);
    b.append([out("b")]);
    expect(texts(b)).toEqual(["a", "gap 40", "b"]);
  });

  it("after resume, skips replayed lines and keeps what is new", () => {
    const b = new LogBuffer();
    b.append([at(1, "one"), at(2, "two")]);
    b.resume();
    b.append([at(1, "one"), at(2, "two"), at(3, "three")]);
    expect(texts(b)).toEqual(["one", "two", "three"]);
    // Once past the old end, later lines are never skipped.
    b.append([at(3, "three again")]);
    expect(texts(b)).toEqual(["one", "two", "three", "three again"]);
  });

  it("resumes correctly when the fraction lengths differ", () => {
    const b = new LogBuffer();
    b.append([at(1, "old", ".5")]);
    b.resume();
    b.append([at(1, "old", ".5"), at(1, "new", ".51")]);
    expect(texts(b)).toEqual(["old", "new"]);
  });

  it("clears lines but still skips them on a later resume", () => {
    const b = new LogBuffer();
    b.append([at(1, "one")]);
    b.clear();
    expect(b.length).toBe(0);
    b.resume();
    b.append([at(1, "one"), at(2, "two")]);
    expect(texts(b)).toEqual(["two"]);
  });

  it("resets everything, including what resume would skip", () => {
    const b = new LogBuffer();
    b.append([at(1, "one")]);
    b.resume();
    b.reset();
    b.append([at(1, "one")]);
    expect(texts(b)).toEqual(["one"]);
  });

  it("bumps its version only when something changed", () => {
    const b = new LogBuffer();
    b.append([at(1, "one")]);
    const v = b.version;
    b.resume();
    b.append([at(1, "one")]);
    expect(b.version).toBe(v);
  });
});

describe("compileQuery and matchRanges", () => {
  it("matches plain text literally and case-insensitively", () => {
    const re = compileQuery("a.b", false) as RegExp;
    expect(matchRanges("A.B axb a.b", re)).toEqual([
      [0, 3],
      [8, 11],
    ]);
  });

  it("supports regular expressions and reports a bad one", () => {
    expect(matchRanges("id=12 id=345", compileQuery("\\d+", true) as RegExp)).toEqual([
      [3, 5],
      [9, 12],
    ]);
    expect(compileQuery("(", true)).toEqual({ error: expect.any(String) });
    expect(compileQuery("", true)).toBeNull();
  });

  it("does not loop on a pattern that matches the empty string", () => {
    expect(matchRanges("abc", compileQuery("x*", true) as RegExp)).toEqual([]);
  });
});

describe("LogMatches", () => {
  it("finds matching lines, keeps up with new ones, and forgets dropped ones", () => {
    const b = new LogBuffer(4);
    const m = new LogMatches(b);
    b.append(["error one", "ok", "\x1b[31mERROR\x1b[0m two"].map(out));
    m.setQuery(compileQuery("error", false) as RegExp);
    expect(m.seqs).toEqual([0, 2]);
    b.append(["ok", "ok", "error three"].map(out));
    m.update();
    expect(m.seqs).toEqual([2, 5]);
  });

  it("matches nothing without a query", () => {
    const b = new LogBuffer();
    b.append([out("error")]);
    const m = new LogMatches(b);
    m.setQuery(null);
    expect(m.seqs).toEqual([]);
  });
});

describe("logText", () => {
  it("writes the lines without escapes, with timestamps when asked", () => {
    const b = new LogBuffer();
    b.append([at(1, "\x1b[32mready\x1b[0m")]);
    b.gap(3);
    expect(logText(b.entries(), false)).toBe("ready\n[3 lines skipped]\n");
    expect(logText(b.entries(), true)).toBe("2026-09-30T10:00:01Z ready\n[3 lines skipped]\n");
  });
});
```

- [ ] **Step 3: Run them to verify they fail**

Run: `cd web && pnpm test src/lib/ansi.test.ts src/lib/logs.test.ts`
Expected: FAIL, 2 files, with `Failed to resolve import "./ansi"` and `Failed to resolve import "./logs"`.

- [ ] **Step 4: Write the ANSI parser**

`web/src/lib/ansi.ts`:

```ts
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
```

- [ ] **Step 5: Write the log buffer and search**

`web/src/lib/logs.ts`:

```ts
import type { LogLine } from "@/api/gen/types.gen";
import { stripAnsi } from "./ansi";

/** Most lines the log viewer keeps; the oldest are dropped past this. */
export const LOG_BUFFER_MAX = 200_000;

export interface LogEntry {
  /** Increases by one per entry for the life of the buffer; never reused. */
  seq: number;
  /** `gap` marks lines the server skipped because the browser fell behind. */
  stream: LogLine["stream"] | "gap";
  /** Docker's RFC 3339 timestamp, or null when the line had none. */
  ts: string | null;
  /** The line after its timestamp, with ANSI escapes left in. */
  text: string;
  /** For a gap: how many lines were skipped. */
  dropped?: number;
}

const TS = /^(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d+)?(?:Z|[+-]\d\d:\d\d)) /;

/** Splits Docker's `timestamps` prefix from a line. */
export function splitTimestamp(text: string): [string | null, string] {
  const m = TS.exec(text);
  return m ? [m[1], text.slice(m[0].length)] : [null, text];
}

/**
 * A key that sorts like the timestamp. Docker trims trailing zeros from the
 * nanoseconds, so `…00.5Z` and `…00.123456789Z` do not compare as strings.
 */
export function timestampKey(ts: string): string {
  const m = /^(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d)(?:\.(\d+))?Z$/.exec(ts);
  if (m) return `${m[1]}.${(m[2] ?? "").padEnd(9, "0")}`;
  // An offset other than Z: millisecond precision is the best Date can do.
  const ms = Date.parse(ts);
  if (Number.isNaN(ms)) return ts;
  const iso = new Date(ms).toISOString();
  return `${iso.slice(0, 19)}.${iso.slice(20, 23)}000000`;
}

/**
 * The log viewer's lines: a ring of at most `max` entries, oldest dropped
 * first. After `resume()`, lines at or before the last timestamp already
 * held are skipped, so a subscription that replays its tail after a
 * reconnect or a container restart adds only what is new.
 */
export class LogBuffer {
  private readonly max: number;
  private ring: LogEntry[] = [];
  private head = 0;
  private nextSeq = 0;
  private lastKey: string | null = null;
  private resumeAfter: string | null = null;
  /** Bumped on every change, for React to notice. */
  version = 0;

  constructor(max = LOG_BUFFER_MAX) {
    this.max = max;
  }

  get length() {
    return this.ring.length;
  }

  /** The seq of the oldest entry held (or of the next one, when empty). */
  get firstSeq() {
    return this.nextSeq - this.ring.length;
  }

  /** Entry `i`, counting from the oldest held. */
  at(i: number): LogEntry | undefined {
    if (i < 0 || i >= this.ring.length) return undefined;
    return this.ring[(this.head + i) % this.ring.length];
  }

  bySeq(seq: number) {
    return this.at(seq - this.firstSeq);
  }

  append(lines: LogLine[]) {
    let changed = false;
    for (const l of lines) {
      const [ts, text] = splitTimestamp(l.text);
      if (ts) {
        const key = timestampKey(ts);
        if (this.resumeAfter !== null) {
          if (key <= this.resumeAfter) continue;
          this.resumeAfter = null;
        }
        this.lastKey = key;
      }
      this.push({ seq: this.nextSeq, stream: l.stream, ts, text });
      changed = true;
    }
    if (changed) this.version++;
  }

  gap(dropped: number) {
    this.push({ seq: this.nextSeq, stream: "gap", ts: null, text: "", dropped });
    this.version++;
  }

  /** Skips replayed lines on the next append (see the class comment). */
  resume() {
    this.resumeAfter = this.lastKey;
  }

  /**
   * Removes every line but remembers the last timestamp, so a resume still
   * skips what was cleared.
   */
  clear() {
    this.ring = [];
    this.head = 0;
    this.version++;
  }

  /** Removes every line and forgets the position: for a new query. */
  reset() {
    this.clear();
    this.lastKey = null;
    this.resumeAfter = null;
  }

  *entries(): Generator<LogEntry> {
    for (let i = 0; i < this.ring.length; i++) yield this.at(i)!;
  }

  private push(e: LogEntry) {
    this.nextSeq++;
    if (this.ring.length < this.max) {
      this.ring.push(e);
    } else {
      this.ring[this.head] = e;
      this.head = (this.head + 1) % this.max;
    }
  }
}

/** A search as typed: plain text (case-insensitive) or a regular expression. */
export function compileQuery(query: string, regex: boolean): RegExp | { error: string } | null {
  if (!query) return null;
  try {
    return new RegExp(regex ? query : query.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"), "gi");
  } catch (e) {
    return { error: (e as Error).message };
  }
}

/** Where `re` matches in `text`, as [start, end) pairs; empty matches are skipped. */
export function matchRanges(text: string, re: RegExp): [number, number][] {
  const out: [number, number][] = [];
  re.lastIndex = 0;
  for (let m = re.exec(text); m; m = re.exec(text)) {
    if (m[0].length === 0) {
      re.lastIndex++;
      continue;
    }
    out.push([m.index, m.index + m[0].length]);
  }
  return out;
}

/**
 * The seqs of lines matching a search, kept up to date as lines arrive:
 * `update()` scans only lines it has not seen and forgets matches the
 * buffer has dropped.
 */
export class LogMatches {
  private readonly buffer: LogBuffer;
  private re: RegExp | null = null;
  private scanned = 0;
  seqs: number[] = [];

  constructor(buffer: LogBuffer) {
    this.buffer = buffer;
  }

  setQuery(re: RegExp | null) {
    this.re = re;
    this.seqs = [];
    this.scanned = this.buffer.firstSeq;
    this.update();
  }

  update() {
    const b = this.buffer;
    const first = b.firstSeq;
    let drop = 0;
    while (drop < this.seqs.length && this.seqs[drop] < first) drop++;
    if (drop) this.seqs = this.seqs.slice(drop);
    if (!this.re) return;
    const end = first + b.length;
    for (let seq = Math.max(this.scanned, first); seq < end; seq++) {
      const e = b.bySeq(seq)!;
      if (e.stream === "gap") continue;
      this.re.lastIndex = 0;
      if (this.re.test(stripAnsi(e.text))) this.seqs.push(seq);
    }
    this.scanned = end;
  }
}

/** The lines as a text file: timestamps if wanted, escapes removed. */
export function logText(entries: Iterable<LogEntry>, timestamps: boolean): string {
  let out = "";
  for (const e of entries) {
    if (e.stream === "gap") out += `[${e.dropped} lines skipped]\n`;
    else out += (timestamps && e.ts ? `${e.ts} ` : "") + stripAnsi(e.text) + "\n";
  }
  return out;
}
```

- [ ] **Step 6: Add the ANSI palette to the theme**

`web/src/styles.css`: the 16 basic colours, darker for the light theme and lighter for the dark one.

```diff
--- a/web/src/styles.css
+++ b/web/src/styles.css
@@ -23,6 +23,23 @@
   --border: oklch(0.922 0 0);
   --input: oklch(0.922 0 0);
   --ring: oklch(0.708 0 0);
+  /* ANSI colours 0–15 for log lines: darker shades that read on a light background. */
+  --ansi-0: #000000;
+  --ansi-1: #c0262d;
+  --ansi-2: #1d7a24;
+  --ansi-3: #8a6d00;
+  --ansi-4: #1f4fbf;
+  --ansi-5: #9b2a9e;
+  --ansi-6: #0b7285;
+  --ansi-7: #6b6b6b;
+  --ansi-8: #555555;
+  --ansi-9: #e03131;
+  --ansi-10: #2b8a3e;
+  --ansi-11: #a87d00;
+  --ansi-12: #3b5bdb;
+  --ansi-13: #ae3ec9;
+  --ansi-14: #1098ad;
+  --ansi-15: #333333;
 }
 
 .dark {
@@ -44,6 +61,22 @@
   --border: oklch(1 0 0 / 10%);
   --input: oklch(1 0 0 / 15%);
   --ring: oklch(0.556 0 0);
+  --ansi-0: #3b3b3b;
+  --ansi-1: #ff6b6b;
+  --ansi-2: #69db7c;
+  --ansi-3: #ffd43b;
+  --ansi-4: #74a7ff;
+  --ansi-5: #e599f7;
+  --ansi-6: #66d9e8;
+  --ansi-7: #d0d0d0;
+  --ansi-8: #808080;
+  --ansi-9: #ff8787;
+  --ansi-10: #8ce99a;
+  --ansi-11: #ffe066;
+  --ansi-12: #a5c4ff;
+  --ansi-13: #f3bcff;
+  --ansi-14: #99e9f2;
+  --ansi-15: #ffffff;
 }
 
 @theme inline {
```

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cd web && pnpm test src/lib/ansi.test.ts src/lib/logs.test.ts`
Expected: PASS, 25 tests.

- [ ] **Step 8: Run all checks**

Run: `cd web && pnpm test && pnpm typecheck && pnpm lint`
Expected: 218 passed; typecheck and lint are clean.

- [ ] **Step 9: Commit**

```bash
git add web/package.json web/pnpm-lock.yaml web/src/lib/ansi.ts web/src/lib/ansi.test.ts web/src/lib/logs.ts web/src/lib/logs.test.ts web/src/styles.css
git commit -m "feat(web): ANSI parsing, the log ring buffer and incremental log search"
```

---

### Task 2: The Logs tab

**Files:**
- Create:
  - `web/src/pages/containers/logs/useLogStream.ts`;
  - `web/src/pages/containers/logs/LogView.tsx`;
  - `web/src/pages/containers/logs/LogsTab.tsx`.
- Modify: `web/src/pages/containers/tabs.ts` and `web/src/pages/containers/ContainerPage.tsx`.
- Test: `web/src/pages/containers/logs/LogsTab.test.tsx`.

**Interfaces:**
- Consumes:
  - Task 1's `LogBuffer`, `parseAnsi`, `cssColor` and `Style`.
  - P3a's `useLive()` (`{ ws }`) and `WsClient.subscribe(spec, handlers)`, with `onData`, `onGap`, `onEnd`, `onError` and `onResubscribe`.
  - P3b's `NativeSelect`, `Checkbox`, `Label` and `Button`.
- Produces `useLogStream.ts`:
  - `interface LogQuery { tail: number | null; since: number | null; stdout: boolean; stderr: boolean }` and `DEFAULT_QUERY` (tail 1000, both streams).
  - `type LogState = { state: "live" } | { state: "ended" } | { state: "error"; message }`.
  - `useLogStream(env, id, query, running): { buffer, version, state, clear }`.
  - It subscribes `{ topic: "logs", env, id, opts: { follow: true, timestamps: true, ...query } }`. A new query means a new buffer. `running` going false→true resubscribes and resumes.
- Produces `LogView.tsx`:
  - `ROW = 20`;
  - `shortTimestamp(ts)`;
  - `<LogView buffer version timestamps wrap follow onFollowChange />`, with `role="log"` named `Container logs`. Task 3 adds `highlight` and `current`.
- Produces `LogsTab.tsx`: `<LogsTab env id running />`. Task 3 adds `name`.
- Accessible names:
  - the selects `Lines` and `Since`;
  - the checkboxes `stdout`, `stderr`, `Timestamps`, `Wrap lines` and `Follow`;
  - the button `Clear`.
- Produces the `logs` tab in `CONTAINER_TABS`.

- [ ] **Step 1: Write the failing tests**

`web/src/pages/containers/logs/LogsTab.test.tsx`:

```tsx
import { act, fireEvent, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { json, mockFetch } from "@/test/fetch";
import { FakeWebSocket } from "@/test/fake-ws";
import { loggedIn, renderApp } from "@/test/app";
import { inspect } from "@/test/docker";

const WEB = inspect({ Id: "c1abcdef0123456789", Name: "/web" });
const STOPPED = inspect({
  Id: "c1abcdef0123456789",
  Name: "/web",
  State: { Status: "exited", Running: false, ExitCode: 0 },
});

const at = (sec: number, text: string, stream = "stdout") => ({
  stream,
  text: `2026-09-30T10:00:${String(sec).padStart(2, "0")}.000000001Z ${text}`,
});

/** Opens the page's socket and returns it with the newest logs subscription. */
async function logsSub(sock = FakeWebSocket.latest(), after = 0) {
  if (sock.readyState !== FakeWebSocket.OPEN) act(() => sock.open());
  let frame: { sub: number; opts: Record<string, unknown> } | undefined;
  await waitFor(() => {
    frame = sock.frames().filter((f) => f.op === "sub" && f.topic === "logs" && f.sub > after).pop();
    expect(frame).toBeTruthy();
  });
  return { sock, sub: frame!.sub, opts: frame!.opts };
}

async function openLogs(inspectBody = WEB) {
  mockFetch({ ...loggedIn, "GET /api/v1/envs/local/containers/c1": { json: inspectBody } });
  const { screen } = renderApp("/envs/local/containers/c1?tab=logs");
  const log = await screen.findByRole("log", { name: "Container logs" });
  return { screen, log, ...(await logsSub()) };
}

describe("logs tab", () => {
  it("follows the log with timestamps requested, and renders colours and stderr", async () => {
    const { log, sock, sub, opts } = await openLogs();
    expect(opts).toEqual({ follow: true, timestamps: true, tail: 1000, since: null, stdout: true, stderr: true });
    act(() =>
      sock.receive({ sub, type: "data", data: [at(1, "\x1b[31mred alert\x1b[0m"), at(2, "disk full", "stderr")] }),
    );
    const red = await within(log).findByText("red alert");
    expect(red).toHaveStyle({ color: "var(--ansi-1)" });
    expect(within(log).getByText("disk full").parentElement).toHaveClass("border-destructive/70");
    expect(within(log).queryByText(/2026-09-30/)).toBeNull();
  });

  it("shows timestamps when asked, without asking the server again", async () => {
    const { screen, log, sock, sub } = await openLogs();
    act(() => sock.receive({ sub, type: "data", data: [at(1, "hello")] }));
    await within(log).findByText("hello");
    await userEvent.click(screen.getByRole("checkbox", { name: "Timestamps" }));
    expect(within(log).getByText("2026-09-30 10:00:01.000Z")).toBeInTheDocument();
    expect(sock.frames().filter((f) => f.topic === "logs")).toHaveLength(1);
  });

  it("starts over when the tail or the streams change", async () => {
    const { screen, log, sock, sub } = await openLogs();
    act(() => sock.receive({ sub, type: "data", data: [at(1, "old line")] }));
    await within(log).findByText("old line");
    await userEvent.selectOptions(screen.getByLabelText("Lines"), "All lines");
    const next = await logsSub(sock, sub);
    expect(next.opts).toMatchObject({ tail: null });
    expect(sock.frames()).toContainEqual({ op: "unsub", sub });
    expect(within(log).queryByText("old line")).toBeNull();
    await userEvent.click(screen.getByRole("checkbox", { name: "stderr" }));
    expect((await logsSub(sock, next.sub)).opts).toMatchObject({ tail: null, stderr: false, stdout: true });
  });

  it("asks for lines since the chosen time", async () => {
    const { screen, sock, sub } = await openLogs();
    const now = Math.floor(Date.now() / 1000);
    await userEvent.selectOptions(screen.getByLabelText("Since"), "Last hour");
    const since = (await logsSub(sock, sub)).opts.since as number;
    expect(since).toBeGreaterThanOrEqual(now - 3600);
    expect(since).toBeLessThanOrEqual(now - 3600 + 5);
  });

  it("resumes after a reconnect without repeating lines", async () => {
    const { log, sock, sub } = await openLogs();
    act(() => sock.receive({ sub, type: "data", data: [at(1, "one"), at(2, "two")] }));
    await within(log).findByText("two");
    act(() => sock.drop());
    await waitFor(() => expect(FakeWebSocket.instances).toHaveLength(2), { timeout: 2000 });
    const again = await logsSub(FakeWebSocket.latest());
    // The same subscription is sent again and the server replays its tail.
    act(() =>
      again.sock.receive({ sub: again.sub, type: "data", data: [at(1, "one"), at(2, "two"), at(3, "three")] }),
    );
    await within(log).findByText("three");
    expect(within(log).getAllByText("one")).toHaveLength(1);
    expect(within(log).getAllByText("two")).toHaveLength(1);
  });

  it("says when the container is stopped, and picks up when it starts again", async () => {
    let body = STOPPED;
    mockFetch({ ...loggedIn, "GET /api/v1/envs/local/containers/c1": () => json(200, body) });
    const { screen } = renderApp("/envs/local/containers/c1?tab=logs");
    const log = await screen.findByRole("log", { name: "Container logs" });
    const { sock, sub } = await logsSub();
    act(() => {
      sock.receive({ sub, type: "data", data: [at(1, "last words")] });
      sock.receive({ sub, type: "end", reason: "eof" });
    });
    expect(await screen.findByText(/The container is not running/)).toBeInTheDocument();
    body = WEB;
    const events = sock.frames().find((f) => f.topic === "events").sub;
    act(() =>
      sock.receive({ sub: events, type: "data", data: { kind: "container", action: "start", id: "c1", attributes: {}, time: 2 } }),
    );
    const next = await logsSub(sock, sub);
    await waitFor(() => expect(screen.queryByText(/The container is not running/)).toBeNull());
    act(() => sock.receive({ sub: next.sub, type: "data", data: [at(1, "last words"), at(5, "back up")] }));
    await within(log).findByText("back up");
    expect(within(log).getAllByText("last words")).toHaveLength(1);
  });

  it("keeps a 250k-line log to the newest 200k and renders only the rows in view", async () => {
    const { log, sock, sub } = await openLogs();
    const lines = Array.from({ length: 250_000 }, (_, i) => ({ stream: "stdout", text: `line ${i}` }));
    act(() => sock.receive({ sub, type: "data", data: lines }));
    // jsdom does not scroll, so the view shows the oldest line kept.
    expect(await within(log).findByText("line 50000")).toBeInTheDocument();
    expect(within(log).queryByText("line 49999")).toBeNull();
    expect(log.querySelectorAll("[data-index]").length).toBeLessThan(100);
    expect((log.firstElementChild as HTMLElement).style.height).toBe(`${200_000 * 20}px`);
  });

  it("marks lines the server skipped", async () => {
    const { log, sock, sub } = await openLogs();
    act(() => {
      sock.receive({ sub, type: "data", data: [at(1, "before")] });
      sock.receive({ sub, type: "gap", dropped: 250 });
    });
    expect(await within(log).findByText(/250 lines skipped/)).toBeInTheDocument();
  });

  it("pauses following when scrolled up and resumes at the bottom", async () => {
    const { screen, log } = await openLogs();
    const follow = screen.getByRole("checkbox", { name: "Follow" });
    expect(follow).toBeChecked();
    Object.defineProperty(log, "scrollHeight", { configurable: true, value: 10_000 });
    Object.defineProperty(log, "clientHeight", { configurable: true, value: 600 });
    const scrollTo = (top: number) => {
      log.scrollTop = top;
      fireEvent.scroll(log);
    };
    scrollTo(9_400);
    scrollTo(4_000);
    expect(follow).not.toBeChecked();
    scrollTo(9_400);
    expect(follow).toBeChecked();
  });

  it("clears the lines shown and keeps them cleared after a reconnect", async () => {
    const { screen, log, sock, sub } = await openLogs();
    act(() => sock.receive({ sub, type: "data", data: [at(1, "noise")] }));
    await within(log).findByText("noise");
    await userEvent.click(screen.getByRole("button", { name: "Clear" }));
    expect(within(log).queryByText("noise")).toBeNull();
    act(() => sock.drop());
    await waitFor(() => expect(FakeWebSocket.instances).toHaveLength(2), { timeout: 2000 });
    const again = await logsSub(FakeWebSocket.latest());
    act(() => again.sock.receive({ sub: again.sub, type: "data", data: [at(1, "noise"), at(2, "fresh")] }));
    await within(log).findByText("fresh");
    expect(within(log).queryByText("noise")).toBeNull();
  });

  it("shows the server's error", async () => {
    const { screen, sock, sub } = await openLogs();
    act(() => sock.receive({ sub, type: "error", code: "not_found", message: "No such container: c1" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("No such container: c1");
  });
});
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cd web && pnpm test src/pages/containers/logs`
Expected: FAIL, 11 tests, each with `Unable to find role="log" and name "Container logs"`.

- [ ] **Step 3: Write the log stream hook**

`web/src/pages/containers/logs/useLogStream.ts`:

```ts
import { useCallback, useEffect, useState } from "react";
import type { LogLine, LogOptions } from "@/api/gen/types.gen";
import { LogBuffer } from "@/lib/logs";
import { useLive } from "@/live/LiveProvider";

/** What the user asked the server for; changing it starts the log over. */
export interface LogQuery {
  /** Lines from the end to start with; null for the whole log. */
  tail: number | null;
  /** Unix seconds; null for no lower bound. */
  since: number | null;
  stdout: boolean;
  stderr: boolean;
}

export const DEFAULT_QUERY: LogQuery = { tail: 1000, since: null, stdout: true, stderr: true };

export type LogState = { state: "live" } | { state: "ended" } | { state: "error"; message: string };

/** How many times `running` has gone from false to true. */
function useStartCount(running: boolean) {
  const [count, setCount] = useState(0);
  const [was, setWas] = useState(running);
  if (running !== was) {
    setWas(running);
    if (running) setCount(count + 1);
  }
  return count;
}

const LIVE: LogState = { state: "live" };

/**
 * Follows a container's log into a `LogBuffer`. Timestamps are always
 * requested: they let the viewer show or hide them without asking again,
 * and let a subscription resume without repeating lines. Changing `query`
 * starts over with a new buffer; a websocket reconnect or the container
 * starting again resumes after the last line held.
 */
export function useLogStream(env: string, id: string, query: LogQuery, running: boolean) {
  const { ws } = useLive();
  const key = JSON.stringify([env, id, query]);
  const [held, setHeld] = useState(() => ({ key, buffer: new LogBuffer() }));
  if (held.key !== key) setHeld({ key, buffer: new LogBuffer() });
  const buffer = held.buffer;
  const [version, setVersion] = useState(0);
  const starts = useStartCount(running);
  // The state of the current subscription; a new one starts out live.
  const [status, setStatus] = useState<{ buffer: LogBuffer; starts: number; state: LogState } | null>(null);
  const state = status && status.buffer === buffer && status.starts === starts ? status.state : LIVE;

  useEffect(() => {
    if (!ws) return;
    const [env, id, query] = JSON.parse(key) as [string, string, LogQuery];
    // A no-op on a new buffer; after a restart, skips what is already held.
    buffer.resume();
    const opts: LogOptions = { follow: true, timestamps: true, ...query };
    const settle = (state: LogState) => setStatus({ buffer, starts, state });
    const { unsubscribe } = ws.subscribe(
      { topic: "logs", env, id, opts },
      {
        onData: (d) => {
          buffer.append(d as LogLine[]);
          setVersion(buffer.version);
        },
        onGap: (n) => {
          buffer.gap(n);
          setVersion(buffer.version);
        },
        onEnd: () => settle({ state: "ended" }),
        onError: (m) => settle({ state: "error", message: m.message }),
        onResubscribe: () => buffer.resume(),
      },
    );
    return unsubscribe;
  }, [ws, key, starts, buffer]);

  const clear = useCallback(() => {
    buffer.clear();
    setVersion(buffer.version);
  }, [buffer]);

  return { buffer, version, state, clear };
}
```

- [ ] **Step 4: Write the virtualised view**

`web/src/pages/containers/logs/LogView.tsx`:

```tsx
import { observeElementRect, useVirtualizer } from "@tanstack/react-virtual";
import { useLayoutEffect, useRef, type CSSProperties } from "react";
import { cssColor, parseAnsi, type Style } from "@/lib/ansi";
import type { LogBuffer, LogEntry } from "@/lib/logs";
import { cn } from "@/lib/utils";

/** Height of one unwrapped line, in pixels. */
export const ROW = 20;

/** The size assumed while the log has no layout (jsdom, or before first paint). */
const FALLBACK_RECT = { width: 1000, height: 600 };

interface Props {
  buffer: LogBuffer;
  /** The buffer's version: a new value re-renders. */
  version: number;
  timestamps: boolean;
  wrap: boolean;
  follow: boolean;
  onFollowChange(follow: boolean): void;
}

/**
 * The log lines, virtualised: only the rows in view are in the DOM, so
 * the full 200k-line buffer scrolls smoothly. While following, it stays
 * scrolled to the newest line; scrolling up pauses following and
 * scrolling back to the bottom resumes it. While paused, the lines in
 * view stay put when old lines are dropped from the top.
 */
export function LogView({ buffer, version, timestamps, wrap, follow, onFollowChange }: Props) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const count = buffer.length;
  // The virtualiser's methods change identity freely, so the compiler leaves this component unmemoised.
  // eslint-disable-next-line react-hooks/incompatible-library
  const v = useVirtualizer({
    count,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW,
    overscan: 30,
    initialRect: FALLBACK_RECT,
    // A zero-height measurement means no layout yet, not an empty viewport.
    observeElementRect: (instance, cb) => observeElementRect(instance, (r) => cb(r.height ? r : FALLBACK_RECT)),
    getItemKey: (i) => buffer.at(i)?.seq ?? i,
  });

  useLayoutEffect(() => {
    if (follow && count > 0) v.scrollToIndex(count - 1, { align: "end" });
  }, [follow, version, count, v]);

  // Keep the view on the same lines while old ones are dropped above it.
  const firstSeq = buffer.firstSeq;
  const prevFirst = useRef(firstSeq);
  useLayoutEffect(() => {
    const dropped = firstSeq - prevFirst.current;
    prevFirst.current = firstSeq;
    const el = scrollRef.current;
    if (!follow && dropped > 0 && el) el.scrollTop = Math.max(0, el.scrollTop - dropped * ROW);
  }, [firstSeq, follow]);

  const lastTop = useRef(0);
  const onScroll = () => {
    const el = scrollRef.current!;
    const up = el.scrollTop < lastTop.current - 1;
    lastTop.current = el.scrollTop;
    const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < ROW * 2;
    // Only the user scrolls up: following only ever scrolls down.
    if (follow && up && !atBottom) onFollowChange(false);
    else if (!follow && !up && atBottom) onFollowChange(true);
  };

  return (
    <div
      ref={scrollRef}
      onScroll={onScroll}
      role="log"
      aria-live="off"
      aria-label="Container logs"
      tabIndex={0}
      className="h-[calc(100vh-20rem)] min-h-80 overflow-auto rounded-md border bg-muted/30 py-1 font-mono text-xs"
    >
      <div className="relative w-full" style={{ height: v.getTotalSize() }}>
        {v.getVirtualItems().map((item) => {
          const e = buffer.at(item.index)!;
          return (
            <div
              key={item.key}
              data-index={item.index}
              ref={wrap ? v.measureElement : undefined}
              className={cn(
                "absolute top-0 left-0 border-l-2 border-transparent px-3 leading-5",
                wrap ? "w-full break-all whitespace-pre-wrap" : "w-max min-w-full whitespace-pre",
                e.stream === "stderr" && "border-destructive/70",
              )}
              style={{ transform: `translateY(${item.start}px)`, height: wrap ? undefined : ROW }}
            >
              <LogRow entry={e} timestamps={timestamps} />
            </div>
          );
        })}
      </div>
    </div>
  );
}

function LogRow({ entry, timestamps }: { entry: LogEntry; timestamps: boolean }) {
  if (entry.stream === "gap") {
    return (
      <span className="text-muted-foreground italic">
        … {entry.dropped} lines skipped because the browser fell behind
      </span>
    );
  }
  return (
    <>
      {timestamps && entry.ts && <span className="mr-3 text-muted-foreground">{shortTimestamp(entry.ts)}</span>}
      {parseAnsi(entry.text).map((s, i) => (
        <span key={i} style={css(s.style)}>
          {s.text}
        </span>
      ))}
    </>
  );
}

/** Docker's timestamp to the millisecond: `2026-09-30 10:00:00.123Z`. */
export function shortTimestamp(ts: string) {
  return ts.replace("T", " ").replace(/(\.\d{3})\d*/, "$1");
}

function css(s: Style): CSSProperties | undefined {
  if (!Object.keys(s).length) return undefined;
  let fg = s.fg === undefined ? undefined : cssColor(s.fg);
  let bg = s.bg === undefined ? undefined : cssColor(s.bg);
  if (s.inverse) [fg, bg] = [bg ?? "var(--background)", fg ?? "var(--foreground)"];
  return {
    color: fg,
    backgroundColor: bg,
    fontWeight: s.bold ? 600 : undefined,
    opacity: s.dim ? 0.7 : undefined,
    fontStyle: s.italic ? "italic" : undefined,
    textDecoration: s.underline ? "underline" : undefined,
  };
}
```

- [ ] **Step 5: Write the tab**

`web/src/pages/containers/logs/LogsTab.tsx`:

```tsx
import { useState, type ReactNode } from "react";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Label } from "@/components/ui/label";
import { NativeSelect } from "@/components/ui/native-select";
import { LogView } from "./LogView";
import { DEFAULT_QUERY, useLogStream, type LogQuery } from "./useLogStream";

const TAILS = [
  ["100", "Last 100 lines"],
  ["1000", "Last 1,000 lines"],
  ["10000", "Last 10,000 lines"],
  ["all", "All lines"],
] as const;

/** Seconds back from now, as select values. */
const SINCE = [
  ["", "Any time"],
  ["300", "Last 5 minutes"],
  ["3600", "Last hour"],
  ["86400", "Last 24 hours"],
] as const;

/** The container's log, live: tail and time range, streams, timestamps, wrapping and follow. */
export function LogsTab({ env, id, running }: { env: string; id: string; running: boolean }) {
  const [query, setQuery] = useState<LogQuery>(DEFAULT_QUERY);
  const [since, setSince] = useState("");
  const [timestamps, setTimestamps] = useState(false);
  const [wrap, setWrap] = useState(false);
  const [follow, setFollow] = useState(true);
  const { buffer, version, state, clear } = useLogStream(env, id, query, running);
  const change = (q: Partial<LogQuery>) => setQuery({ ...query, ...q });

  return (
    <div className="grid gap-3">
      <div className="flex flex-wrap items-center gap-x-5 gap-y-2">
        <NativeSelect
          aria-label="Lines"
          className="w-44"
          value={query.tail === null ? "all" : String(query.tail)}
          onChange={(e) => change({ tail: e.target.value === "all" ? null : Number(e.target.value) })}
        >
          {TAILS.map(([v, label]) => (
            <option key={v} value={v}>
              {label}
            </option>
          ))}
        </NativeSelect>
        <NativeSelect
          aria-label="Since"
          className="w-40"
          value={since}
          onChange={(e) => {
            setSince(e.target.value);
            change({ since: e.target.value ? Math.floor(Date.now() / 1000) - Number(e.target.value) : null });
          }}
        >
          {SINCE.map(([v, label]) => (
            <option key={v} value={v}>
              {label}
            </option>
          ))}
        </NativeSelect>
        <Toggle id="logs-stdout" checked={query.stdout} onChange={(stdout) => change({ stdout })}>
          stdout
        </Toggle>
        <Toggle id="logs-stderr" checked={query.stderr} onChange={(stderr) => change({ stderr })}>
          stderr
        </Toggle>
        <Toggle id="logs-timestamps" checked={timestamps} onChange={setTimestamps}>
          Timestamps
        </Toggle>
        <Toggle id="logs-wrap" checked={wrap} onChange={setWrap}>
          Wrap lines
        </Toggle>
        <Toggle id="logs-follow" checked={follow} onChange={setFollow}>
          Follow
        </Toggle>
        <Button variant="outline" size="sm" onClick={clear}>
          Clear
        </Button>
      </div>
      <LogStatus state={state} running={running} empty={buffer.length === 0} />
      <LogView
        buffer={buffer}
        version={version}
        timestamps={timestamps}
        wrap={wrap}
        follow={follow}
        onFollowChange={setFollow}
      />
    </div>
  );
}

function Toggle({
  id,
  checked,
  onChange,
  children,
}: {
  id: string;
  checked: boolean;
  onChange(v: boolean): void;
  children: ReactNode;
}) {
  return (
    <div className="flex items-center gap-2">
      <Checkbox id={id} checked={checked} onCheckedChange={(v) => onChange(v === true)} />
      <Label htmlFor={id} className="font-normal">
        {children}
      </Label>
    </div>
  );
}

function LogStatus({ state, running, empty }: { state: ReturnType<typeof useLogStream>["state"]; running: boolean; empty: boolean }) {
  if (state.state === "error") {
    return (
      <p role="alert" className="text-sm text-destructive">
        {state.message}
      </p>
    );
  }
  if (!running) {
    return (
      <p className="text-sm text-muted-foreground">
        The container is not running. New lines appear here when it starts again.
      </p>
    );
  }
  if (state.state === "ended") return <p className="text-sm text-muted-foreground">The log stream ended.</p>;
  if (empty) return <p className="text-sm text-muted-foreground">No log lines yet.</p>;
  return null;
}
```

- [ ] **Step 6: Add the tab to the container page**

`web/src/pages/containers/tabs.ts`:

```diff
--- a/web/src/pages/containers/tabs.ts
+++ b/web/src/pages/containers/tabs.ts
@@ -1,3 +1,3 @@
 /** The container page's tabs, in order; the router validates `?tab=` against them. */
-export const CONTAINER_TABS = ["overview", "inspect", "env", "mounts", "networks", "files"] as const;
+export const CONTAINER_TABS = ["overview", "inspect", "env", "mounts", "networks", "logs", "files"] as const;
 export type ContainerTab = (typeof CONTAINER_TABS)[number];
```

`web/src/pages/containers/ContainerPage.tsx`:

```diff
--- a/web/src/pages/containers/ContainerPage.tsx
+++ b/web/src/pages/containers/ContainerPage.tsx
@@ -22,6 +22,7 @@ import { isSecretKey, maskEnv, splitEnv } from "@/lib/secrets";
 import { FormError } from "@/pages/AuthLayout";
 import { ACTIONS, actionsFor, RemoveDialog } from "./actions";
 import { FilesTab } from "./FilesTab";
+import { LogsTab } from "./logs/LogsTab";
 import type { ContainerTab } from "./tabs";
 
 const FROM = "/authed/shell/envs/$env/containers/$id";
@@ -56,6 +57,7 @@ export function ContainerPage() {
           <TabsTrigger value="env">Env</TabsTrigger>
           <TabsTrigger value="mounts">Mounts</TabsTrigger>
           <TabsTrigger value="networks">Networks</TabsTrigger>
+          <TabsTrigger value="logs">Logs</TabsTrigger>
           <TabsTrigger value="files">Files</TabsTrigger>
         </TabsList>
         <TabsContent value="overview">
@@ -73,6 +75,9 @@ export function ContainerPage() {
         <TabsContent value="networks">
           <Networks c={c} />
         </TabsContent>
+        <TabsContent value="logs">
+          <LogsTab env={env} id={id} running={c.State.Running === true} />
+        </TabsContent>
         <TabsContent value="files">
           <FilesTab env={env} id={id} path={path} />
         </TabsContent>
```

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cd web && pnpm test src/pages/containers/logs`
Expected: PASS, 11 tests.

- [ ] **Step 8: Run all checks**

Run: `cd web && pnpm test && pnpm typecheck && pnpm lint`
Expected: 229 passed; typecheck and lint are clean. The `eslint-disable-next-line react-hooks/incompatible-library` in `LogView.tsx` is needed: without it, lint warns that the React Compiler skips the component.

- [ ] **Step 9: Commit**

```bash
git add web/src/pages/containers
git commit -m "feat(web): live container logs with follow, tail, time range, streams and resume"
```

---

### Task 3: Log search and download

**Files:**
- Create: `web/src/lib/download.ts`.
- Modify:
  - `web/src/lib/logs.ts` (`sync`, `markSegments`);
  - `web/src/pages/containers/logs/LogView.tsx` (highlighting and the current match);
  - `web/src/pages/containers/logs/LogsTab.tsx` (the search row and downloads);
  - `web/src/pages/containers/ContainerPage.tsx` (passes `name`).
- Test: `web/src/lib/logs.test.ts` and `web/src/pages/containers/logs/LogsTab.test.tsx`.

**Interfaces:**
- Consumes: Task 1's `LogMatches`, `compileQuery`, `matchRanges` and `logText`, and Task 2's `LogView`, `LogsTab` and `useLogStream`.
- Produces `@/lib/logs`:
  - `LogMatches.sync(re): number[]`, which starts over for a new `re` and catches up for the same one;
  - `markSegments(segments, ranges): (Segment & { mark: boolean })[]`.
- Produces `@/lib/download`: `saveText(filename, text)`.
- Produces `LogView`: new props `highlight?: RegExp | null` and `current?: number | null` (a seq, scrolled into view and marked).
- Produces `LogsTab`: the new prop `name`, used for the file name `<name>.log`.
- Accessible names:
  - the input `Search logs`;
  - the checkbox `Regex`;
  - the buttons `Previous match`, `Next match`, `Download shown` and `Download full log` (`Preparing…` while busy).
- The match count is a polite live region: `N matches`, `i of N`, `No matches` or `Invalid regular expression`.

- [ ] **Step 1: Write the failing tests**

`web/src/lib/logs.test.ts`:

```diff
--- a/web/src/lib/logs.test.ts
+++ b/web/src/lib/logs.test.ts
@@ -1,5 +1,15 @@
 import type { LogLine } from "@/api/gen/types.gen";
-import { compileQuery, LogBuffer, LogMatches, logText, matchRanges, splitTimestamp, timestampKey } from "./logs";
+import { parseAnsi } from "./ansi";
+import {
+  compileQuery,
+  LogBuffer,
+  LogMatches,
+  logText,
+  markSegments,
+  matchRanges,
+  splitTimestamp,
+  timestampKey,
+} from "./logs";
 
 const out = (text: string): LogLine => ({ stream: "stdout", text });
 const at = (sec: number, text: string, frac = "") =>
@@ -130,6 +140,17 @@ describe("LogMatches", () => {
     expect(m.seqs).toEqual([2, 5]);
   });
 
+  it("starts over when synced with a new query and catches up with the same one", () => {
+    const b = new LogBuffer();
+    b.append(["error", "warn"].map(out));
+    const m = new LogMatches(b);
+    const err = compileQuery("error", false) as RegExp;
+    expect(m.sync(err)).toEqual([0]);
+    b.append([out("error again")]);
+    expect(m.sync(err)).toEqual([0, 2]);
+    expect(m.sync(compileQuery("warn", false) as RegExp)).toEqual([1]);
+  });
+
   it("matches nothing without a query", () => {
     const b = new LogBuffer();
     b.append([out("error")]);
@@ -139,6 +160,28 @@ describe("LogMatches", () => {
   });
 });
 
+describe("markSegments", () => {
+  it("marks matches across colour changes and keeps each part's style", () => {
+    const segs = parseAnsi("ab\x1b[31mcd\x1b[0mef");
+    expect(markSegments(segs, [[1, 3]])).toEqual([
+      { text: "a", style: {}, mark: false },
+      { text: "b", style: {}, mark: true },
+      { text: "c", style: { fg: 1 }, mark: true },
+      { text: "d", style: { fg: 1 }, mark: false },
+      { text: "ef", style: {}, mark: false },
+    ]);
+  });
+
+  it("marks several matches in one segment", () => {
+    expect(markSegments(parseAnsi("xaxa"), [[1, 2], [3, 4]]).map((s) => [s.text, s.mark])).toEqual([
+      ["x", false],
+      ["a", true],
+      ["x", false],
+      ["a", true],
+    ]);
+  });
+});
+
 describe("logText", () => {
   it("writes the lines without escapes, with timestamps when asked", () => {
     const b = new LogBuffer();
```

`web/src/pages/containers/logs/LogsTab.test.tsx`: search and download tests.

```diff
--- a/web/src/pages/containers/logs/LogsTab.test.tsx
+++ b/web/src/pages/containers/logs/LogsTab.test.tsx
@@ -1,6 +1,6 @@
 import { act, fireEvent, waitFor, within } from "@testing-library/react";
 import userEvent from "@testing-library/user-event";
-import { describe, expect, it } from "vitest";
+import { describe, expect, it, vi } from "vitest";
 import { json, mockFetch } from "@/test/fetch";
 import { FakeWebSocket } from "@/test/fake-ws";
 import { loggedIn, renderApp } from "@/test/app";
@@ -176,3 +176,108 @@ describe("logs tab", () => {
     expect(await screen.findByRole("alert")).toHaveTextContent("No such container: c1");
   });
 });
+
+describe("logs search", () => {
+  it("highlights matches, counts them and moves between them, pausing follow", async () => {
+    const { screen, log, sock, sub } = await openLogs();
+    act(() =>
+      sock.receive({
+        sub,
+        type: "data",
+        data: [at(1, "GET /health 200"), at(2, "\x1b[31mERROR\x1b[0m db down"), at(3, "ok"), at(4, "error: retry")],
+      }),
+    );
+    await within(log).findByText("ok");
+    await userEvent.type(screen.getByLabelText("Search logs"), "error");
+    expect(screen.getByText("2 matches")).toBeInTheDocument();
+    const marks = log.querySelectorAll("mark");
+    expect([...marks].map((m) => m.textContent)).toEqual(["ERROR", "error"]);
+    // The match keeps its colour.
+    expect(marks[0]).toHaveStyle({ color: "var(--ansi-1)" });
+    await userEvent.keyboard("{Enter}");
+    expect(screen.getByText("1 of 2")).toBeInTheDocument();
+    expect(screen.getByRole("checkbox", { name: "Follow" })).not.toBeChecked();
+    await userEvent.click(screen.getByRole("button", { name: "Next match" }));
+    expect(screen.getByText("2 of 2")).toBeInTheDocument();
+    await userEvent.click(screen.getByRole("button", { name: "Next match" }));
+    expect(screen.getByText("1 of 2")).toBeInTheDocument();
+    await userEvent.click(screen.getByRole("button", { name: "Previous match" }));
+    expect(screen.getByText("2 of 2")).toBeInTheDocument();
+  });
+
+  it("counts matches in lines that arrive after the search", async () => {
+    const { screen, log, sock, sub } = await openLogs();
+    await userEvent.type(screen.getByLabelText("Search logs"), "timeout");
+    expect(screen.getByText("No matches")).toBeInTheDocument();
+    act(() => sock.receive({ sub, type: "data", data: [at(1, "request timeout")] }));
+    await within(log).findByText("timeout");
+    expect(screen.getByText("1 match")).toBeInTheDocument();
+  });
+
+  it("searches with a regular expression and says when it is invalid", async () => {
+    const { screen, log, sock, sub } = await openLogs();
+    act(() => sock.receive({ sub, type: "data", data: [at(1, "took 120ms"), at(2, "took 9ms")] }));
+    await within(log).findByText("took 9ms");
+    await userEvent.click(screen.getByRole("checkbox", { name: "Regex" }));
+    const box = screen.getByLabelText("Search logs");
+    await userEvent.type(box, "\\d{{3}ms");
+    expect(screen.getByText("1 match")).toBeInTheDocument();
+    await userEvent.clear(box);
+    await userEvent.type(box, "(");
+    expect(screen.getByText("Invalid regular expression")).toBeInTheDocument();
+  });
+});
+
+describe("logs download", () => {
+  function captureDownloads() {
+    const saved: { name: string; blob: Blob }[] = [];
+    let last: Blob | null = null;
+    // jsdom has no object URLs; each test file runs in its own environment.
+    URL.createObjectURL = (b: Blob) => {
+      last = b;
+      return "blob:log";
+    };
+    URL.revokeObjectURL = () => {};
+    vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(function (this: HTMLAnchorElement) {
+      saved.push({ name: this.download, blob: last! });
+    });
+    return saved;
+  }
+
+  it("saves the lines shown, as plain text", async () => {
+    const saved = captureDownloads();
+    const { screen, log, sock, sub } = await openLogs();
+    act(() => sock.receive({ sub, type: "data", data: [at(1, "\x1b[32mready\x1b[0m")] }));
+    await within(log).findByText("ready");
+    await userEvent.click(screen.getByRole("button", { name: "Download shown" }));
+    expect(saved).toHaveLength(1);
+    expect(saved[0].name).toBe("web.log");
+    expect(await saved[0].blob.text()).toBe("ready\n");
+  });
+
+  it("fetches the whole log for a full download", async () => {
+    const saved = captureDownloads();
+    const { screen, sock, sub } = await openLogs();
+    await userEvent.click(screen.getByRole("checkbox", { name: "Timestamps" }));
+    await userEvent.click(screen.getByRole("button", { name: "Download full log" }));
+    const full = await logsSub(sock, sub);
+    expect(full.opts).toEqual({ follow: false, tail: null, since: null, timestamps: true, stdout: true, stderr: true });
+    expect(screen.getByRole("button", { name: /Preparing/ })).toBeDisabled();
+    act(() => {
+      sock.receive({ sub: full.sub, type: "data", data: [at(1, "first ever")] });
+      sock.receive({ sub: full.sub, type: "end", reason: "eof" });
+    });
+    await waitFor(() => expect(saved).toHaveLength(1));
+    expect(await saved[0].blob.text()).toBe("2026-09-30T10:00:01.000000001Z first ever\n");
+    expect(screen.getByRole("button", { name: "Download full log" })).toBeEnabled();
+  });
+
+  it("says why a full download failed", async () => {
+    captureDownloads();
+    const { screen, sock, sub } = await openLogs();
+    await userEvent.click(screen.getByRole("button", { name: "Download full log" }));
+    const full = await logsSub(sock, sub);
+    act(() => sock.receive({ sub: full.sub, type: "error", code: "env_unavailable", message: "Docker is down" }));
+    expect(await screen.findByText("Could not download the log: Docker is down")).toBeInTheDocument();
+  });
+});
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cd web && pnpm test src/lib/logs.test.ts src/pages/containers/logs`
Expected: FAIL, 9 tests:
- `m.sync is not a function` and `markSegments is not a function`;
- `Unable to find a label with the text of: Search logs`;
- `Unable to find an accessible element with the role "button" and name "Download shown"` or `"Download full log"`.

- [ ] **Step 3: Add syncing and segment marking**

`web/src/lib/logs.ts`:

```diff
--- a/web/src/lib/logs.ts
+++ b/web/src/lib/logs.ts
@@ -1,5 +1,5 @@
 import type { LogLine } from "@/api/gen/types.gen";
-import { stripAnsi } from "./ansi";
+import { stripAnsi, type Segment } from "./ansi";
 
 /** Most lines the log viewer keeps; the oldest are dropped past this. */
 export const LOG_BUFFER_MAX = 200_000;
@@ -177,6 +177,13 @@ export class LogMatches {
     this.buffer = buffer;
   }
 
+  /** Brings the matches up to date for `re`, starting over if it changed. */
+  sync(re: RegExp | null) {
+    if (re === this.re) this.update();
+    else this.setQuery(re);
+    return this.seqs;
+  }
+
   setQuery(re: RegExp | null) {
     this.re = re;
     this.seqs = [];
@@ -202,6 +209,31 @@ export class LogMatches {
   }
 }
 
+/**
+ * Splits styled segments at match boundaries, so matches can be marked
+ * without losing their colours. `ranges` index the segments' joined text.
+ */
+export function markSegments(segments: Segment[], ranges: [number, number][]): (Segment & { mark: boolean })[] {
+  const out: (Segment & { mark: boolean })[] = [];
+  let pos = 0;
+  let r = 0;
+  for (const seg of segments) {
+    let i = 0;
+    while (i < seg.text.length) {
+      const at = pos + i;
+      while (r < ranges.length && ranges[r][1] <= at) r++;
+      const range = ranges[r];
+      const inside = range !== undefined && range[0] <= at;
+      const stop = inside ? range[1] : range ? range[0] : Infinity;
+      const len = Math.min(seg.text.length - i, stop - at);
+      out.push({ text: seg.text.slice(i, i + len), style: seg.style, mark: inside });
+      i += len;
+    }
+    pos += seg.text.length;
+  }
+  return out;
+}
+
 /** The lines as a text file: timestamps if wanted, escapes removed. */
 export function logText(entries: Iterable<LogEntry>, timestamps: boolean): string {
   let out = "";
```

`web/src/lib/download.ts`:

```ts
/** Saves `text` as a file through the browser's download. */
export function saveText(filename: string, text: string) {
  const url = URL.createObjectURL(new Blob([text], { type: "text/plain;charset=utf-8" }));
  const a = document.createElement("a");
  a.href = url;
  a.download = filename;
  document.body.append(a);
  a.click();
  a.remove();
  // Revoked later: some browsers start the download after click() returns.
  setTimeout(() => URL.revokeObjectURL(url), 10_000);
}
```

- [ ] **Step 4: Highlight matches in the view**

`web/src/pages/containers/logs/LogView.tsx`:

```diff
--- a/web/src/pages/containers/logs/LogView.tsx
+++ b/web/src/pages/containers/logs/LogView.tsx
@@ -1,7 +1,7 @@
 import { observeElementRect, useVirtualizer } from "@tanstack/react-virtual";
 import { useLayoutEffect, useRef, type CSSProperties } from "react";
 import { cssColor, parseAnsi, type Style } from "@/lib/ansi";
-import type { LogBuffer, LogEntry } from "@/lib/logs";
+import { markSegments, matchRanges, type LogBuffer, type LogEntry } from "@/lib/logs";
 import { cn } from "@/lib/utils";
 
 /** Height of one unwrapped line, in pixels. */
@@ -18,6 +18,10 @@ interface Props {
   wrap: boolean;
   follow: boolean;
   onFollowChange(follow: boolean): void;
+  /** The search to highlight, if any. */
+  highlight?: RegExp | null;
+  /** The seq of the current match: scrolled into view and marked. */
+  current?: number | null;
 }
 
 /**
@@ -27,7 +31,7 @@ interface Props {
  * scrolling back to the bottom resumes it. While paused, the lines in
  * view stay put when old lines are dropped from the top.
  */
-export function LogView({ buffer, version, timestamps, wrap, follow, onFollowChange }: Props) {
+export function LogView({ buffer, version, timestamps, wrap, follow, onFollowChange, highlight, current }: Props) {
   const scrollRef = useRef<HTMLDivElement>(null);
   const count = buffer.length;
   // The virtualiser's methods change identity freely, so the compiler leaves this component unmemoised.
@@ -47,6 +51,10 @@ export function LogView({ buffer, version, timestamps, wrap, follow, onFollowCha
     if (follow && count > 0) v.scrollToIndex(count - 1, { align: "end" });
   }, [follow, version, count, v]);
 
+  useLayoutEffect(() => {
+    if (current != null && buffer.bySeq(current)) v.scrollToIndex(current - buffer.firstSeq, { align: "center" });
+  }, [current, buffer, v]);
+
   // Keep the view on the same lines while old ones are dropped above it.
   const firstSeq = buffer.firstSeq;
   const prevFirst = useRef(firstSeq);
@@ -90,10 +98,11 @@ export function LogView({ buffer, version, timestamps, wrap, follow, onFollowCha
                 "absolute top-0 left-0 border-l-2 border-transparent px-3 leading-5",
                 wrap ? "w-full break-all whitespace-pre-wrap" : "w-max min-w-full whitespace-pre",
                 e.stream === "stderr" && "border-destructive/70",
+                e.seq === current && "bg-accent",
               )}
               style={{ transform: `translateY(${item.start}px)`, height: wrap ? undefined : ROW }}
             >
-              <LogRow entry={e} timestamps={timestamps} />
+              <LogRow entry={e} timestamps={timestamps} highlight={highlight ?? null} />
             </div>
           );
         })}
@@ -102,7 +111,7 @@ export function LogView({ buffer, version, timestamps, wrap, follow, onFollowCha
   );
 }
 
-function LogRow({ entry, timestamps }: { entry: LogEntry; timestamps: boolean }) {
+function LogRow({ entry, timestamps, highlight }: { entry: LogEntry; timestamps: boolean; highlight: RegExp | null }) {
   if (entry.stream === "gap") {
     return (
       <span className="text-muted-foreground italic">
@@ -110,14 +119,22 @@ function LogRow({ entry, timestamps }: { entry: LogEntry; timestamps: boolean })
       </span>
     );
   }
+  const segments = parseAnsi(entry.text);
+  const ranges = highlight ? matchRanges(segments.map((s) => s.text).join(""), highlight) : [];
   return (
     <>
       {timestamps && entry.ts && <span className="mr-3 text-muted-foreground">{shortTimestamp(entry.ts)}</span>}
-      {parseAnsi(entry.text).map((s, i) => (
-        <span key={i} style={css(s.style)}>
-          {s.text}
-        </span>
-      ))}
+      {markSegments(segments, ranges).map((s, i) =>
+        s.mark ? (
+          <mark key={i} style={css(s.style)} className="rounded-sm bg-yellow-300/80 text-inherit dark:bg-yellow-500/50">
+            {s.text}
+          </mark>
+        ) : (
+          <span key={i} style={css(s.style)}>
+            {s.text}
+          </span>
+        ),
+      )}
     </>
   );
 }
```

- [ ] **Step 5: Add search and downloads to the tab**

`web/src/pages/containers/logs/LogsTab.tsx`:

```diff
--- a/web/src/pages/containers/logs/LogsTab.tsx
+++ b/web/src/pages/containers/logs/LogsTab.tsx
@@ -1,8 +1,15 @@
-import { useState, type ReactNode } from "react";
+import { ChevronDown, ChevronUp, Download } from "lucide-react";
+import { useEffect, useMemo, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
+import { toast } from "sonner";
+import type { LogLine } from "@/api/gen/types.gen";
 import { Button } from "@/components/ui/button";
 import { Checkbox } from "@/components/ui/checkbox";
+import { Input } from "@/components/ui/input";
 import { Label } from "@/components/ui/label";
 import { NativeSelect } from "@/components/ui/native-select";
+import { saveText } from "@/lib/download";
+import { compileQuery, LogBuffer, LogMatches, logText } from "@/lib/logs";
+import { useLive } from "@/live/LiveProvider";
 import { LogView } from "./LogView";
 import { DEFAULT_QUERY, useLogStream, type LogQuery } from "./useLogStream";
 
@@ -21,8 +28,11 @@ const SINCE = [
   ["86400", "Last 24 hours"],
 ] as const;
 
-/** The container's log, live: tail and time range, streams, timestamps, wrapping and follow. */
-export function LogsTab({ env, id, running }: { env: string; id: string; running: boolean }) {
+/**
+ * The container's log, live: tail and time range, streams, timestamps,
+ * wrapping and follow, search, and download.
+ */
+export function LogsTab({ env, id, name, running }: { env: string; id: string; name: string; running: boolean }) {
   const [query, setQuery] = useState<LogQuery>(DEFAULT_QUERY);
   const [since, setSince] = useState("");
   const [timestamps, setTimestamps] = useState(false);
@@ -30,6 +40,10 @@ export function LogsTab({ env, id, running }: { env: string; id: string; running
   const [follow, setFollow] = useState(true);
   const { buffer, version, state, clear } = useLogStream(env, id, query, running);
   const change = (q: Partial<LogQuery>) => setQuery({ ...query, ...q });
+  const search = useSearch(buffer, version);
+  const jump = (dir: 1 | -1) => {
+    if (search.jump(dir) !== null) setFollow(false);
+  };
 
   return (
     <div className="grid gap-3">
@@ -80,6 +94,38 @@ export function LogsTab({ env, id, running }: { env: string; id: string; running
           Clear
         </Button>
       </div>
+      <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
+        <Input
+          aria-label="Search logs"
+          placeholder="Search"
+          className="w-64"
+          value={search.text}
+          onChange={(e) => search.setText(e.target.value)}
+          onKeyDown={(e: KeyboardEvent) => {
+            if (e.key !== "Enter") return;
+            e.preventDefault();
+            jump(e.shiftKey ? -1 : 1);
+          }}
+        />
+        <Toggle id="logs-regex" checked={search.regex} onChange={search.setRegex}>
+          Regex
+        </Toggle>
+        <Button variant="outline" size="icon" aria-label="Previous match" disabled={!search.seqs.length} onClick={() => jump(-1)}>
+          <ChevronUp />
+        </Button>
+        <Button variant="outline" size="icon" aria-label="Next match" disabled={!search.seqs.length} onClick={() => jump(1)}>
+          <ChevronDown />
+        </Button>
+        <span aria-live="polite" className={search.error ? "text-sm text-destructive" : "text-sm text-muted-foreground"}>
+          {search.summary}
+        </span>
+        <div className="ml-auto flex gap-2">
+          <Button variant="outline" size="sm" onClick={() => saveText(`${name}.log`, logText(buffer.entries(), timestamps))}>
+            <Download /> Download shown
+          </Button>
+          <FullDownload env={env} id={id} name={name} query={query} timestamps={timestamps} />
+        </div>
+      </div>
       <LogStatus state={state} running={running} empty={buffer.length === 0} />
       <LogView
         buffer={buffer}
@@ -88,6 +134,8 @@ export function LogsTab({ env, id, running }: { env: string; id: string; running
         wrap={wrap}
         follow={follow}
         onFollowChange={setFollow}
+        highlight={search.re}
+        current={search.current}
       />
     </div>
   );
@@ -133,3 +181,96 @@ function LogStatus({ state, running, empty }: { state: ReturnType<typeof useLogS
   if (empty) return <p className="text-sm text-muted-foreground">No log lines yet.</p>;
   return null;
 }
+
+/**
+ * The search box's state: the matching lines' seqs, kept current as lines
+ * arrive, and the match the user has moved to.
+ */
+function useSearch(buffer: LogBuffer, version: number) {
+  const [text, setText] = useState("");
+  const [regex, setRegex] = useState(false);
+  const [current, setCurrent] = useState<{ text: string; regex: boolean; seq: number } | null>(null);
+  const compiled = useMemo(() => compileQuery(text, regex), [text, regex]);
+  const re = compiled instanceof RegExp ? compiled : null;
+  const matches = useMemo(() => new LogMatches(buffer), [buffer]);
+  // eslint-disable-next-line react-hooks/exhaustive-deps -- `version` marks new lines in the buffer
+  const seqs = useMemo(() => [...matches.sync(re)], [matches, re, version]);
+  const seq = current && current.text === text && current.regex === regex ? current.seq : null;
+  const index = seq === null ? -1 : seqs.indexOf(seq);
+
+  /** Moves to the next (1) or previous (-1) match, wrapping around; returns its seq. */
+  const jump = (dir: 1 | -1) => {
+    if (!seqs.length) return null;
+    let next: number;
+    if (seq === null) next = dir === 1 ? seqs[0] : seqs[seqs.length - 1];
+    else if (dir === 1) next = seqs.find((s) => s > seq) ?? seqs[0];
+    else next = seqs.findLast((s) => s < seq) ?? seqs[seqs.length - 1];
+    setCurrent({ text, regex, seq: next });
+    return next;
+  };
+
+  const error = compiled && !(compiled instanceof RegExp) ? compiled.error : null;
+  const summary = error
+    ? "Invalid regular expression"
+    : !re
+      ? ""
+      : !seqs.length
+        ? "No matches"
+        : index >= 0
+          ? `${index + 1} of ${seqs.length}`
+          : `${seqs.length} ${seqs.length === 1 ? "match" : "matches"}`;
+  return { text, setText, regex, setRegex, re, seqs, current: seq, jump, error, summary };
+}
+
+/** Fetches the whole log (not only the lines held) and saves it. */
+function FullDownload({
+  env,
+  id,
+  name,
+  query,
+  timestamps,
+}: {
+  env: string;
+  id: string;
+  name: string;
+  query: LogQuery;
+  timestamps: boolean;
+}) {
+  const { ws } = useLive();
+  const [busy, setBusy] = useState(false);
+  // Leaving the page abandons a download still in progress.
+  const pending = useRef<(() => void) | null>(null);
+  useEffect(() => () => pending.current?.(), []);
+  const start = () => {
+    if (!ws) return;
+    setBusy(true);
+    const all = new LogBuffer(Infinity);
+    pending.current = ws.subscribe(
+      {
+        topic: "logs",
+        env,
+        id,
+        opts: { follow: false, tail: null, since: null, timestamps: true, stdout: query.stdout, stderr: query.stderr },
+      },
+      {
+        onData: (d) => all.append(d as LogLine[]),
+        onGap: (n) => all.gap(n),
+        onEnd: () => {
+          pending.current = null;
+          setBusy(false);
+          saveText(`${name}.log`, logText(all.entries(), timestamps));
+        },
+        onError: (m) => {
+          pending.current = null;
+          setBusy(false);
+          toast.error(`Could not download the log: ${m.message}`);
+        },
+      },
+    ).unsubscribe;
+  };
+  return (
+    <Button variant="outline" size="sm" disabled={busy} onClick={start}>
+      <Download /> {busy ? "Preparing…" : "Download full log"}
+    </Button>
+  );
+}
```

`web/src/pages/containers/ContainerPage.tsx`:

```diff
--- a/web/src/pages/containers/ContainerPage.tsx
+++ b/web/src/pages/containers/ContainerPage.tsx
@@ -76,7 +76,7 @@ export function ContainerPage() {
           <Networks c={c} />
         </TabsContent>
         <TabsContent value="logs">
-          <LogsTab env={env} id={id} running={c.State.Running === true} />
+          <LogsTab env={env} id={id} name={name} running={c.State.Running === true} />
         </TabsContent>
         <TabsContent value="files">
           <FilesTab env={env} id={id} path={path} />
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cd web && pnpm test src/lib/logs.test.ts src/pages/containers/logs`
Expected: PASS, 36 tests.

- [ ] **Step 7: Run all checks**

Run: `cd web && pnpm test && pnpm typecheck && pnpm lint`
Expected: 238 passed; typecheck and lint are clean. The `exhaustive-deps` disable on the `seqs` memo is needed: `version` is how the memo learns that lines arrived.

- [ ] **Step 8: Commit**

```bash
git add web/src/lib web/src/pages/containers
git commit -m "feat(web): log search with highlighting and match navigation, and log downloads"
```

---

### Task 4: The Terminal tab

**Files:**
- Create:
  - `web/src/pages/containers/terminal/TerminalView.tsx`;
  - `web/src/pages/containers/terminal/TerminalTab.tsx`.
- Modify: `web/src/live/ws.ts`, `web/src/pages/containers/tabs.ts` and `web/src/pages/containers/ContainerPage.tsx`.
- Test: `web/src/live/ws.test.ts` and `web/src/pages/containers/terminal/TerminalTab.test.tsx`.

**Interfaces:**
- Consumes:
  - P3a's `WsClient.send`, `WsClient.sendBinary(sub, bytes)` and the `onBinary` handler;
  - P3b's `splitArgs` (throws `unclosed " quote`);
  - `ExecOptions` from `@/api/gen/types.gen` (`{ cmd?, user?, cols?, rows? }`).
- Produces `@/live/ws`:
  - `interface SubOptions { resubscribe?: boolean }`;
  - `subscribe(spec, handlers, opts?)`;
  - `SubHandlers.onLost?()`, called when the socket closes on a subscription made with `resubscribe: false`. The subscription is dropped, not resent. A subscription still queued before any socket opened is unaffected.
- Produces `TerminalView.tsx`:
  - `type SessionEnd = { kind: "exited"; code } | { kind: "lost" } | { kind: "error"; message }`;
  - `<TerminalView env id opts={{ cmd, user }} onStarted onEnd />`, labelled `Terminal`.
- Produces `TerminalTab.tsx`: `<TerminalTab env id running />`. It is loaded lazily by `ContainerPage`, so xterm.js is its own chunk.
- Accessible names:
  - the select `Shell`, with the options `Auto (bash, then sh)`, `bash`, `sh` and `Custom command`;
  - the inputs `Command` and `User`;
  - the buttons `Connect`, `Connect again` and `Disconnect`;
  - the `role="status"` line.
- Produces the `terminal` tab.

- [ ] **Step 1: Write the failing tests**

`web/src/live/ws.test.ts`:

```diff
--- a/web/src/live/ws.test.ts
+++ b/web/src/live/ws.test.ts
@@ -77,6 +77,33 @@ describe("WsClient", () => {
     expect(sock.frames().filter((f) => f.op === "unsub")).toHaveLength(1);
   });
 
+  it("ends a subscription made without resubscribe when the connection drops", () => {
+    const ws = client();
+    const sock = FakeWebSocket.latest();
+    sock.open();
+    const onLost = vi.fn();
+    const kept = vi.fn();
+    ws.subscribe({ topic: "exec", env: "local", id: "c1" }, { onData: () => {}, onLost }, { resubscribe: false });
+    ws.subscribe({ topic: "events", env: "local" }, { onData: () => {}, onLost: kept });
+    sock.drop();
+    expect(onLost).toHaveBeenCalledOnce();
+    vi.advanceTimersByTime(100);
+    FakeWebSocket.latest().open();
+    expect(FakeWebSocket.latest().frames()).toEqual([{ op: "sub", sub: 2, topic: "events", env: "local" }]);
+    expect(kept).not.toHaveBeenCalled();
+  });
+
+  it("still sends a no-resubscribe subscription made before the socket first opened", () => {
+    const ws = client();
+    const onLost = vi.fn();
+    ws.subscribe({ topic: "exec", env: "local", id: "c1" }, { onData: () => {}, onLost }, { resubscribe: false });
+    FakeWebSocket.latest().drop(); // the first attempt failed
+    vi.advanceTimersByTime(100);
+    FakeWebSocket.latest().open();
+    expect(onLost).not.toHaveBeenCalled();
+    expect(FakeWebSocket.latest().frames()).toEqual([{ op: "sub", sub: 1, topic: "exec", env: "local", id: "c1" }]);
+  });
+
   it("reconnects with backoff and resubscribes every active subscription", () => {
     const delays: number[] = [];
     const onReconnect = vi.fn();
```

`web/src/pages/containers/terminal/TerminalTab.test.tsx`:

```tsx
import { act, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { mockFetch } from "@/test/fetch";
import { FakeWebSocket } from "@/test/fake-ws";
import { loggedIn, renderApp } from "@/test/app";
import { inspect } from "@/test/docker";

// xterm.js needs a real layout and canvas; the tests drive a stand-in.
const { FakeTerminal } = vi.hoisted(() => {
  class FakeTerminal {
    static all: FakeTerminal[] = [];
    cols = 80;
    rows = 24;
    options: Record<string, unknown>;
    written = "";
    selection = "";
    disposed = false;
    keyHandler: ((e: Partial<KeyboardEvent>) => boolean) | null = null;
    private dataCb: ((s: string) => void) | null = null;
    private resizeCb: ((s: { cols: number; rows: number }) => void) | null = null;
    constructor(opts: Record<string, unknown>) {
      this.options = { ...opts };
      FakeTerminal.all.push(this);
    }
    static latest() {
      return FakeTerminal.all[FakeTerminal.all.length - 1];
    }
    loadAddon() {}
    open() {}
    focus() {}
    write(b: Uint8Array | string) {
      this.written += typeof b === "string" ? b : new TextDecoder().decode(b);
    }
    onData(cb: (s: string) => void) {
      this.dataCb = cb;
      return { dispose: () => (this.dataCb = null) };
    }
    onResize(cb: (s: { cols: number; rows: number }) => void) {
      this.resizeCb = cb;
      return { dispose: () => (this.resizeCb = null) };
    }
    attachCustomKeyEventHandler(h: (e: Partial<KeyboardEvent>) => boolean) {
      this.keyHandler = h;
    }
    hasSelection() {
      return this.selection !== "";
    }
    getSelection() {
      return this.selection;
    }
    dispose() {
      this.disposed = true;
    }
    // Test controls.
    type(s: string) {
      this.dataCb?.(s);
    }
    resizeTo(cols: number, rows: number) {
      this.cols = cols;
      this.rows = rows;
      this.resizeCb?.({ cols, rows });
    }
  }
  return { FakeTerminal };
});
vi.mock("@xterm/xterm", () => ({ Terminal: FakeTerminal }));
vi.mock("@xterm/addon-fit", () => ({ FitAddon: class { fit() {} } }));

const RUNNING = inspect({ Id: "c1abcdef0123456789", Name: "/web" });
const STOPPED = inspect({ Id: "c1abcdef0123456789", Name: "/web", State: { Status: "exited", Running: false, ExitCode: 0 } });

/** A binary exec frame: `[u32 sub, big-endian][bytes]`. */
function frame(sub: number, text: string) {
  const bytes = new TextEncoder().encode(text);
  const out = new Uint8Array(4 + bytes.length);
  new DataView(out.buffer).setUint32(0, sub);
  out.set(bytes, 4);
  return out.buffer;
}

async function openTerminal(body = RUNNING) {
  FakeTerminal.all = [];
  mockFetch({ ...loggedIn, "GET /api/v1/envs/local/containers/c1": { json: body } });
  const { screen } = renderApp("/envs/local/containers/c1?tab=terminal");
  await screen.findByLabelText("Shell");
  const sock = FakeWebSocket.latest();
  act(() => sock.open());
  return { screen, sock };
}

/** The newest exec subscription sent on `sock`. */
async function execSub(sock: FakeWebSocket, after = 0) {
  let f: { sub: number; opts: Record<string, unknown> } | undefined;
  await waitFor(() => {
    f = sock.frames().filter((x) => x.op === "sub" && x.topic === "exec" && x.sub > after).pop();
    expect(f).toBeTruthy();
  });
  return f!;
}

describe("terminal tab", () => {
  it("starts nothing until asked, then connects with the auto shell at the terminal's size", async () => {
    const { screen, sock } = await openTerminal();
    expect(screen.getByRole("status")).toHaveTextContent("Choose a shell and connect");
    expect(sock.frames().some((f) => f.topic === "exec")).toBe(false);
    await userEvent.click(screen.getByRole("button", { name: "Connect" }));
    const { sub, opts } = await execSub(sock);
    expect(opts).toEqual({ cmd: null, user: null, cols: 80, rows: 24 });
    expect(screen.getByRole("status")).toHaveTextContent("Starting…");
    act(() => sock.receive(frame(sub, "root@web:/# ")));
    expect(FakeTerminal.latest().written).toBe("root@web:/# ");
    expect(screen.getByRole("status")).toHaveTextContent("Connected");
  });

  it("sends keystrokes as binary frames and tells the process when the terminal resizes", async () => {
    const { screen, sock } = await openTerminal();
    await userEvent.click(screen.getByRole("button", { name: "Connect" }));
    const { sub } = await execSub(sock);
    act(() => FakeTerminal.latest().type("ls\r"));
    const sent = sock.sent.at(-1) as Uint8Array;
    expect(new DataView(sent.buffer).getUint32(0)).toBe(sub);
    expect(new TextDecoder().decode(sent.slice(4))).toBe("ls\r");
    act(() => FakeTerminal.latest().resizeTo(120, 40));
    expect(sock.frames().at(-1)).toEqual({ op: "resize", sub, cols: 120, rows: 40 });
  });

  it("runs a custom command as the chosen user", async () => {
    const { screen, sock } = await openTerminal();
    await userEvent.selectOptions(screen.getByLabelText("Shell"), "Custom command");
    await userEvent.type(screen.getByLabelText("Command"), 'python3 -c "print(1)"');
    await userEvent.type(screen.getByLabelText("User"), "www-data");
    await userEvent.click(screen.getByRole("button", { name: "Connect" }));
    expect((await execSub(sock)).opts).toMatchObject({ cmd: ["python3", "-c", "print(1)"], user: "www-data" });
  });

  it("runs sh when chosen", async () => {
    const { screen, sock } = await openTerminal();
    await userEvent.selectOptions(screen.getByLabelText("Shell"), "sh");
    await userEvent.click(screen.getByRole("button", { name: "Connect" }));
    expect((await execSub(sock)).opts).toMatchObject({ cmd: ["sh"] });
  });

  it("names a command it cannot parse and starts nothing", async () => {
    const { screen, sock } = await openTerminal();
    await userEvent.selectOptions(screen.getByLabelText("Shell"), "Custom command");
    await userEvent.type(screen.getByLabelText("Command"), 'echo "oops');
    await userEvent.click(screen.getByRole("button", { name: "Connect" }));
    expect(screen.getByRole("alert")).toHaveTextContent('Command: unclosed " quote');
    expect(sock.frames().some((f) => f.topic === "exec")).toBe(false);
  });

  it("reports the exit code and connects again in a new terminal", async () => {
    const { screen, sock } = await openTerminal();
    await userEvent.click(screen.getByRole("button", { name: "Connect" }));
    const { sub } = await execSub(sock);
    act(() => sock.receive({ sub, type: "end", reason: "exited", exit_code: 130 }));
    expect(await screen.findByText("The process exited with code 130.")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Connect again" }));
    await execSub(sock, sub);
    expect(FakeTerminal.all).toHaveLength(2);
    expect(FakeTerminal.all[0].disposed).toBe(true);
  });

  it("ends the session when the connection drops, and does not start another on reconnect", async () => {
    const { screen, sock } = await openTerminal();
    await userEvent.click(screen.getByRole("button", { name: "Connect" }));
    await execSub(sock);
    act(() => sock.drop());
    expect(await screen.findByText("The connection dropped, which ended the session.")).toBeInTheDocument();
    await waitFor(() => expect(FakeWebSocket.instances).toHaveLength(2), { timeout: 2000 });
    const next = FakeWebSocket.latest();
    act(() => next.open());
    await waitFor(() => expect(next.frames().some((f) => f.topic === "events")).toBe(true));
    expect(next.frames().some((f) => f.topic === "exec")).toBe(false);
    expect(screen.getByRole("button", { name: "Connect again" })).toBeEnabled();
  });

  it("disconnects on request", async () => {
    const { screen, sock } = await openTerminal();
    await userEvent.click(screen.getByRole("button", { name: "Connect" }));
    const { sub } = await execSub(sock);
    await userEvent.click(screen.getByRole("button", { name: "Disconnect" }));
    expect(sock.frames()).toContainEqual({ op: "unsub", sub });
    expect(FakeTerminal.latest().disposed).toBe(true);
    expect(screen.getByRole("button", { name: "Connect" })).toBeInTheDocument();
  });

  it("shows why the exec failed", async () => {
    const { screen, sock } = await openTerminal();
    await userEvent.click(screen.getByRole("button", { name: "Connect" }));
    const { sub } = await execSub(sock);
    act(() => sock.receive({ sub, type: "error", code: "upstream", message: "container is paused" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("container is paused");
  });

  it("cannot connect to a stopped container", async () => {
    const { screen } = await openTerminal(STOPPED);
    expect(screen.getByRole("status")).toHaveTextContent("Start the container to open a terminal.");
    expect(screen.getByRole("button", { name: "Connect" })).toBeDisabled();
  });

  it("copies the selection with Ctrl+Shift+C and leaves Ctrl+C to the process", async () => {
    const writeText = vi.fn(async () => {});
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
    const { screen, sock } = await openTerminal();
    await userEvent.click(screen.getByRole("button", { name: "Connect" }));
    await execSub(sock);
    const term = FakeTerminal.latest();
    term.selection = "hello";
    expect(term.keyHandler!({ type: "keydown", ctrlKey: true, shiftKey: true, code: "KeyC" })).toBe(false);
    expect(writeText).toHaveBeenCalledWith("hello");
    expect(term.keyHandler!({ type: "keydown", ctrlKey: true, shiftKey: false, code: "KeyC" })).toBe(true);
  });
});
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cd web && pnpm test src/live/ws.test.ts src/pages/containers/terminal`
Expected: FAIL, 12 tests.
- `ends a subscription made without resubscribe…` fails with `expected "vi.fn()" to be called once, but got 0 times`.
- The terminal tests fail with `Unable to find a label with the text of: Shell`.
- `still sends a no-resubscribe subscription made before the socket first opened` already passes. It guards against the new option breaking queued subscriptions.

- [ ] **Step 3: Let a subscription opt out of resubscribing**

`web/src/live/ws.ts`:

```diff
--- a/web/src/live/ws.ts
+++ b/web/src/live/ws.ts
@@ -21,6 +21,20 @@ export interface SubHandlers {
   onError?(msg: ErrorMsg): void;
   /** The connection was re-established and the subscription sent again. */
   onResubscribe?(): void;
+  /**
+   * The connection dropped and, because it was made with
+   * `resubscribe: false`, the subscription is over.
+   */
+  onLost?(): void;
+}
+
+export interface SubOptions {
+  /**
+   * Send the subscription again after a reconnect (the default). An exec
+   * session cannot resume, so a terminal passes false: resending it would
+   * silently start a new process.
+   */
+  resubscribe?: boolean;
 }
 
 /**
@@ -60,6 +74,7 @@ export function wsUrl(loc: Location = window.location) {
 interface Active {
   spec: SubSpec;
   handlers: SubHandlers;
+  resubscribe: boolean;
 }
 
 /**
@@ -89,9 +104,13 @@ export class WsClient {
     this.connect();
   }
 
-  subscribe(spec: SubSpec, handlers: SubHandlers): { sub: number; unsubscribe: () => void } {
+  subscribe(
+    spec: SubSpec,
+    handlers: SubHandlers,
+    { resubscribe = true }: SubOptions = {},
+  ): { sub: number; unsubscribe: () => void } {
     const sub = this.nextSub++;
-    this.subs.set(sub, { spec, handlers });
+    this.subs.set(sub, { spec, handlers, resubscribe });
     this.sendSub(sub, spec);
     return {
       sub,
@@ -156,6 +175,15 @@ export class WsClient {
       if (this.socket !== socket) return;
       this.socket = null;
       if (this.stopped) return;
+      // Only a subscription that reached an open socket is lost; one still
+      // queued is sent once a socket opens.
+      if (socketOpened) {
+        for (const [sub, a] of this.subs) {
+          if (a.resubscribe) continue;
+          this.subs.delete(sub);
+          a.handlers.onLost?.();
+        }
+      }
       if (!socketOpened) this.opts.onConnectFailed?.();
       if (this.attempt === 0) this.opts.onState?.("reconnecting");
       const delay = (this.opts.backoff ?? defaultBackoff)(this.attempt++);
```

- [ ] **Step 4: Write the terminal**

`web/src/pages/containers/terminal/TerminalView.tsx`:

```tsx
import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import { useEffect, useRef } from "react";
import type { ExecOptions } from "@/api/gen/types.gen";
import { useLive } from "@/live/LiveProvider";

export type SessionEnd =
  | { kind: "exited"; code: number | null }
  | { kind: "lost" }
  | { kind: "error"; message: string };

/** A terminal is dark in both themes; the palette is the dark log palette. */
const THEME = {
  background: "#0c0c0d",
  foreground: "#e5e5e5",
  cursor: "#e5e5e5",
  selectionBackground: "#3b5bdb80",
  black: "#3b3b3b",
  red: "#ff6b6b",
  green: "#69db7c",
  yellow: "#ffd43b",
  blue: "#74a7ff",
  magenta: "#e599f7",
  cyan: "#66d9e8",
  white: "#d0d0d0",
  brightBlack: "#808080",
  brightRed: "#ff8787",
  brightGreen: "#8ce99a",
  brightYellow: "#ffe066",
  brightBlue: "#a5c4ff",
  brightMagenta: "#f3bcff",
  brightCyan: "#99e9f2",
  brightWhite: "#ffffff",
};

interface Props {
  env: string;
  id: string;
  /** Command and user; the size comes from the terminal. */
  opts: Pick<ExecOptions, "cmd" | "user">;
  onStarted(): void;
  onEnd(end: SessionEnd): void;
}

/**
 * One exec session in an xterm.js terminal. Output arrives as binary
 * frames; keystrokes go back as binary frames; the terminal fits its box
 * and tells the process its new size. The session never resumes after a
 * dropped connection: resending the subscription would start a new
 * process, so it reports `lost` instead. Unmounting ends the session.
 */
export function TerminalView({ env, id, opts, onStarted, onEnd }: Props) {
  const { ws } = useLive();
  const box = useRef<HTMLDivElement>(null);
  const handlers = useRef({ onStarted, onEnd });
  useEffect(() => {
    handlers.current = { onStarted, onEnd };
  });
  const optsKey = JSON.stringify(opts);

  useEffect(() => {
    const el = box.current;
    if (!ws || !el) return;
    const term = new Terminal({
      cursorBlink: true,
      fontSize: 13,
      fontFamily: "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
      theme: THEME,
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(el);
    fit.fit();
    // Ctrl+Shift+C copies the selection; plain Ctrl+C still interrupts.
    term.attachCustomKeyEventHandler((e) => {
      if (e.type === "keydown" && e.ctrlKey && e.shiftKey && e.code === "KeyC" && term.hasSelection()) {
        void navigator.clipboard?.writeText(term.getSelection());
        return false;
      }
      return true;
    });

    let started = false;
    let over = false;
    const end = (e: SessionEnd) => {
      over = true;
      term.options.cursorBlink = false;
      handlers.current.onEnd(e);
    };
    const { sub, unsubscribe } = ws.subscribe(
      {
        topic: "exec",
        env,
        id,
        opts: { ...(JSON.parse(optsKey) as Props["opts"]), cols: term.cols, rows: term.rows },
      },
      {
        onData: () => {},
        onBinary: (bytes) => {
          if (!started) {
            started = true;
            handlers.current.onStarted();
          }
          term.write(bytes);
        },
        onEnd: (m) => end({ kind: "exited", code: m.exit_code ?? null }),
        onError: (m) => end({ kind: "error", message: m.message }),
        onLost: () => end({ kind: "lost" }),
      },
      { resubscribe: false },
    );
    const encoder = new TextEncoder();
    const input = term.onData((s) => {
      if (!over) ws.sendBinary(sub, encoder.encode(s));
    });
    const resize = term.onResize(({ cols, rows }) => {
      if (!over) ws.send({ op: "resize", sub, cols, rows });
    });
    const observer = new ResizeObserver(() => fit.fit());
    observer.observe(el);
    term.focus();
    return () => {
      observer.disconnect();
      input.dispose();
      resize.dispose();
      unsubscribe();
      term.dispose();
    };
  }, [ws, env, id, optsKey]);

  return <div ref={box} aria-label="Terminal" className="h-[calc(100vh-20rem)] min-h-80 rounded-md bg-[#0c0c0d] p-2" />;
}
```

`web/src/pages/containers/terminal/TerminalTab.tsx`:

```tsx
import { useState, type FormEvent } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { NativeSelect } from "@/components/ui/native-select";
import { splitArgs } from "@/lib/args";
import { TerminalView, type SessionEnd } from "./TerminalView";

type Shell = "auto" | "bash" | "sh" | "custom";

interface Session {
  /** A new number mounts a new terminal. */
  n: number;
  cmd: string[] | null;
  user: string | null;
}

/**
 * A shell in the container. Nothing runs until the user connects: every
 * exec is audited, and switching tabs should not start one.
 */
export function TerminalTab({ env, id, running }: { env: string; id: string; running: boolean }) {
  const [shell, setShell] = useState<Shell>("auto");
  const [command, setCommand] = useState("");
  const [user, setUser] = useState("");
  const [formError, setFormError] = useState<string | null>(null);
  const [session, setSession] = useState<Session | null>(null);
  const [started, setStarted] = useState(false);
  const [ended, setEnded] = useState<SessionEnd | null>(null);

  const connect = (e?: FormEvent) => {
    e?.preventDefault();
    let cmd: string[] | null = null;
    if (shell === "bash" || shell === "sh") cmd = [shell];
    if (shell === "custom") {
      try {
        cmd = splitArgs(command);
      } catch (err) {
        setFormError(`Command: ${(err as Error).message}`);
        return;
      }
      if (!cmd.length) {
        setFormError("Command: enter the command to run");
        return;
      }
    }
    setFormError(null);
    setStarted(false);
    setEnded(null);
    setSession({ n: (session?.n ?? 0) + 1, cmd, user: user.trim() || null });
  };

  const live = session && !ended;
  return (
    <div className="grid gap-3">
      <form onSubmit={connect} className="flex flex-wrap items-end gap-3">
        <div className="grid gap-1.5">
          <Label htmlFor="term-shell">Shell</Label>
          <NativeSelect
            id="term-shell"
            className="w-52"
            value={shell}
            disabled={!!live}
            onChange={(e) => setShell(e.target.value as Shell)}
          >
            <option value="auto">Auto (bash, then sh)</option>
            <option value="bash">bash</option>
            <option value="sh">sh</option>
            <option value="custom">Custom command</option>
          </NativeSelect>
        </div>
        {shell === "custom" && (
          <div className="grid gap-1.5">
            <Label htmlFor="term-command">Command</Label>
            <Input
              id="term-command"
              className="w-64 font-mono"
              placeholder="python3 -i"
              value={command}
              disabled={!!live}
              onChange={(e) => setCommand(e.target.value)}
            />
          </div>
        )}
        <div className="grid gap-1.5">
          <Label htmlFor="term-user">User</Label>
          <Input
            id="term-user"
            className="w-40"
            placeholder="image default"
            value={user}
            disabled={!!live}
            onChange={(e) => setUser(e.target.value)}
          />
        </div>
        {live ? (
          <Button type="button" variant="outline" onClick={() => setSession(null)}>
            Disconnect
          </Button>
        ) : (
          <Button type="submit" disabled={!running}>
            {session ? "Connect again" : "Connect"}
          </Button>
        )}
      </form>
      {formError && (
        <p role="alert" className="text-sm text-destructive">
          {formError}
        </p>
      )}
      <Status running={running} session={session} started={started} ended={ended} />
      {session && (
        <TerminalView
          key={session.n}
          env={env}
          id={id}
          opts={{ cmd: session.cmd, user: session.user }}
          onStarted={() => setStarted(true)}
          onEnd={setEnded}
        />
      )}
    </div>
  );
}

function Status({
  running,
  session,
  started,
  ended,
}: {
  running: boolean;
  session: Session | null;
  started: boolean;
  ended: SessionEnd | null;
}) {
  if (ended?.kind === "error") {
    return (
      <p role="alert" className="text-sm text-destructive">
        {ended.message}
      </p>
    );
  }
  const text =
    ended?.kind === "exited"
      ? ended.code === null
        ? "The process exited."
        : `The process exited with code ${ended.code}.`
      : ended?.kind === "lost"
        ? "The connection dropped, which ended the session."
        : !running && !session
          ? "Start the container to open a terminal."
          : !session
            ? "Choose a shell and connect. The session is recorded in the audit log."
            : started
              ? "Connected. Ctrl+Shift+C copies the selection."
              : "Starting…";
  return (
    <p role="status" className="text-sm text-muted-foreground">
      {text}
    </p>
  );
}
```

- [ ] **Step 5: Add the tab, loaded lazily**

`web/src/pages/containers/tabs.ts`:

```diff
--- a/web/src/pages/containers/tabs.ts
+++ b/web/src/pages/containers/tabs.ts
@@ -1,3 +1,3 @@
 /** The container page's tabs, in order; the router validates `?tab=` against them. */
-export const CONTAINER_TABS = ["overview", "inspect", "env", "mounts", "networks", "logs", "files"] as const;
+export const CONTAINER_TABS = ["overview", "inspect", "env", "mounts", "networks", "logs", "terminal", "files"] as const;
 export type ContainerTab = (typeof CONTAINER_TABS)[number];
```

`web/src/pages/containers/ContainerPage.tsx`:

```diff
--- a/web/src/pages/containers/ContainerPage.tsx
+++ b/web/src/pages/containers/ContainerPage.tsx
@@ -1,6 +1,6 @@
 import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
 import { Link, useNavigate, useParams, useSearch } from "@tanstack/react-router";
-import { useState, type FormEvent } from "react";
+import { lazy, Suspense, useState, type FormEvent } from "react";
 import { containerQuery, keys } from "@/api/queries";
 import { containerAction, renameContainer } from "@/api/gen/sdk.gen";
 import type { ContainerActionName } from "@/api/gen/types.gen";
@@ -27,6 +27,9 @@ import type { ContainerTab } from "./tabs";
 
 const FROM = "/authed/shell/envs/$env/containers/$id";
 
+// xterm.js is large; it loads only when the Terminal tab opens.
+const TerminalTab = lazy(() => import("./terminal/TerminalTab").then((m) => ({ default: m.TerminalTab })));
+
 export function ContainerPage() {
   const { env, id } = useParams({ from: FROM });
   const { tab = "overview", path = "/" } = useSearch({ from: FROM });
@@ -58,6 +61,7 @@ export function ContainerPage() {
           <TabsTrigger value="mounts">Mounts</TabsTrigger>
           <TabsTrigger value="networks">Networks</TabsTrigger>
           <TabsTrigger value="logs">Logs</TabsTrigger>
+          <TabsTrigger value="terminal">Terminal</TabsTrigger>
           <TabsTrigger value="files">Files</TabsTrigger>
         </TabsList>
         <TabsContent value="overview">
@@ -78,6 +82,11 @@ export function ContainerPage() {
         <TabsContent value="logs">
           <LogsTab env={env} id={id} name={name} running={c.State.Running === true} />
         </TabsContent>
+        <TabsContent value="terminal">
+          <Suspense fallback={<p className="text-sm text-muted-foreground">Loading…</p>}>
+            <TerminalTab env={env} id={id} running={c.State.Running === true} />
+          </Suspense>
+        </TabsContent>
         <TabsContent value="files">
           <FilesTab env={env} id={id} path={path} />
         </TabsContent>
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cd web && pnpm test src/live/ws.test.ts src/pages/containers/terminal`
Expected: PASS, 29 tests.

- [ ] **Step 7: Run all checks, and look at the chunks**

Run: `cd web && pnpm test && pnpm typecheck && pnpm lint && pnpm build`
Expected:
- 251 passed; typecheck and lint are clean.
- The build lists `TerminalTab-*.js` (about 337 kB) and `TerminalTab-*.css` as their own chunks, and `ContainerPage-*.js` stays near 50 kB.

- [ ] **Step 8: Commit**

```bash
git add web/src/live/ws.ts web/src/live/ws.test.ts web/src/pages/containers
git commit -m "feat(web): container terminal over exec, with resize, shell choice and no silent resume"
```

---

### Task 5: The stats store and the list's sparklines

**Files:**
- Create:
  - `web/src/live/stats.ts`;
  - `web/src/components/Sparkline.tsx`;
  - `web/src/components/useInView.ts`.
- Modify:
  - `web/src/live/LiveProvider.tsx`;
  - `web/src/lib/format.ts`;
  - `web/src/pages/containers/ContainersPage.tsx`;
  - `web/src/components/DataTable.tsx`.
- Test:
  - `web/src/live/stats.test.ts`;
  - `web/src/lib/lib.test.ts`;
  - `web/src/pages/containers/ContainersPage.test.tsx`.

**Interfaces:**
- Consumes:
  - `StatsSample` from `@/api/gen/types.gen`: `{ time (ms), cpu_percent, memory_used, memory_limit, net_rx, net_tx, block_read, block_write, pids }`;
  - `WsClient.subscribe`;
  - `formatBytes`.
- Produces `@/live/stats`:
  - `STATS_HISTORY = 120`;
  - `interface StatsPoint extends StatsSample { net_rx_rate, net_tx_rate, block_read_rate, block_write_rate: number | null }`;
  - `toPoint(sample, prev)`;
  - `class StatsStore(ws, graceMs = 5000)`, with `get(env, id): StatsPoint[]`, which returns the same array until a sample arrives, and `subscribe(env, id, listener): () => void`.
- Produces `@/live/LiveProvider`:
  - `useLive()` now also returns `stats: StatsStore | null`;
  - `useContainerStats(env, id, enabled): StatsPoint[]`.
- Produces:
  - `@/components/Sparkline`: `<Sparkline values max label />`, an `svg role="img"` named `label`.
  - `@/components/useInView`: `useInView<T>(): [ref, inView]`. It is always in view where there is no `IntersectionObserver`.
  - `@/lib/format`: `formatPercent(n)`.
- The containers list gains the `CPU` and `Memory` display columns. Their sparklines are named `CPU of <name>` and `Memory of <name>`.

- [ ] **Step 1: Write the failing tests**

`web/src/live/stats.test.ts`:

```ts
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { StatsSample } from "@/api/gen/types.gen";
import { FakeWebSocket } from "@/test/fake-ws";
import { StatsStore, STATS_HISTORY, toPoint } from "./stats";
import { WsClient } from "./ws";

const sample = (time: number, over: Partial<StatsSample> = {}): StatsSample => ({
  time,
  cpu_percent: 1,
  memory_used: 100,
  memory_limit: 1000,
  net_rx: 0,
  net_tx: 0,
  block_read: 0,
  block_write: 0,
  pids: 1,
  ...over,
});

describe("toPoint", () => {
  it("turns running totals into per-second rates", () => {
    const p = toPoint(sample(3000, { net_rx: 5000, block_write: 800 }), sample(1000, { net_rx: 1000, block_write: 0 }));
    expect(p.net_rx_rate).toBe(2000);
    expect(p.block_write_rate).toBe(400);
  });

  it("has no rate for the first sample and never a negative one", () => {
    expect(toPoint(sample(1000), undefined).net_rx_rate).toBeNull();
    expect(toPoint(sample(2000, { net_tx: 10 }), sample(1000, { net_tx: 900 })).net_tx_rate).toBe(0);
    expect(toPoint(sample(1000), sample(1000)).net_tx_rate).toBeNull();
  });
});

describe("StatsStore", () => {
  let ws: WsClient;
  let sock: FakeWebSocket;
  beforeEach(() => {
    vi.useFakeTimers();
    FakeWebSocket.instances = [];
    ws = new WsClient({ url: "ws://test", WebSocketImpl: FakeWebSocket as unknown as typeof WebSocket });
    ws.start();
    sock = FakeWebSocket.latest();
    sock.open();
  });
  afterEach(() => vi.useRealTimers());

  const subs = () => sock.frames().filter((f) => f.op === "sub" && f.topic === "stats");

  it("shares one subscription between readers and keeps a bounded history", () => {
    const store = new StatsStore(ws);
    const a = vi.fn();
    const b = vi.fn();
    store.subscribe("local", "c1", a);
    store.subscribe("local", "c1", b);
    expect(subs()).toEqual([{ op: "sub", sub: 1, topic: "stats", env: "local", id: "c1" }]);
    for (let i = 0; i < STATS_HISTORY + 5; i++) sock.receive({ sub: 1, type: "data", data: sample(i * 1000) });
    expect(a).toHaveBeenCalledTimes(STATS_HISTORY + 5);
    expect(b).toHaveBeenCalledTimes(STATS_HISTORY + 5);
    const points = store.get("local", "c1");
    expect(points).toHaveLength(STATS_HISTORY);
    expect(points.at(-1)!.time).toBe((STATS_HISTORY + 4) * 1000);
    expect(store.get("local", "c1")).toBe(points);
  });

  it("keeps the subscription through a brief gap between readers, then ends it", () => {
    const store = new StatsStore(ws, 1000);
    const stop = store.subscribe("local", "c1", () => {});
    sock.receive({ sub: 1, type: "data", data: sample(1000) });
    stop();
    vi.advanceTimersByTime(500);
    store.subscribe("local", "c1", () => {})();
    expect(subs()).toHaveLength(1);
    expect(store.get("local", "c1")).toHaveLength(1);
    vi.advanceTimersByTime(1000);
    expect(sock.frames().at(-1)).toEqual({ op: "unsub", sub: 1 });
    expect(store.get("local", "c1")).toHaveLength(0);
  });

  it("subscribes again for a new reader after the stream ended", () => {
    const store = new StatsStore(ws);
    const stop = store.subscribe("local", "c1", () => {});
    sock.receive({ sub: 1, type: "end", reason: "ended" });
    stop();
    store.subscribe("local", "c1", () => {});
    expect(subs().map((f) => f.sub)).toEqual([1, 2]);
  });
});
```

`web/src/lib/lib.test.ts`:

```diff
--- a/web/src/lib/lib.test.ts
+++ b/web/src/lib/lib.test.ts
@@ -1,7 +1,7 @@
 import { describe, expect, it } from "vitest";
 import { ApiError } from "@/api/client";
 import { errorMessage } from "./errors";
-import { formatBytes } from "./format";
+import { formatBytes, formatPercent } from "./format";
 
 describe("formatBytes", () => {
   it("scales by 1000 with one decimal", () => {
@@ -18,6 +18,16 @@ describe("formatBytes", () => {
   });
 });
 
+describe("formatPercent", () => {
+  it("keeps one decimal below 10% and none above", () => {
+    expect(formatPercent(0.04)).toBe("0.0%");
+    expect(formatPercent(3.26)).toBe("3.3%");
+    expect(formatPercent(12.6)).toBe("13%");
+    expect(formatPercent(250)).toBe("250%");
+    expect(formatPercent(Number.NaN)).toBe("—");
+  });
+});
+
 describe("errorMessage", () => {
   it("adds the request id to internal errors only", () => {
     expect(errorMessage(new ApiError(500, { code: "internal", message: "internal error", request_id: "r9" }))).toBe(
```

`web/src/pages/containers/ContainersPage.test.tsx`: also moves the `act` and `FakeWebSocket` imports to the top.

```diff
--- a/web/src/pages/containers/ContainersPage.test.tsx
+++ b/web/src/pages/containers/ContainersPage.test.tsx
@@ -1,7 +1,8 @@
-import { waitFor, within } from "@testing-library/react";
+import { act, waitFor, within } from "@testing-library/react";
 import userEvent from "@testing-library/user-event";
-import { describe, expect, it } from "vitest";
+import { describe, expect, it, vi } from "vitest";
 import { json, mockFetch } from "@/test/fetch";
+import { FakeWebSocket } from "@/test/fake-ws";
 import { loggedIn, renderApp } from "@/test/app";
 import { container } from "@/test/docker";
 
@@ -103,8 +104,6 @@ describe("containers list", () => {
     const { screen } = renderApp("/envs/local/containers");
     await waitFor(() => expect(rows(screen)).toHaveLength(2));
     list = [...LIST, container({ id: "c3", name: "cache" })];
-    const { act } = await import("@testing-library/react");
-    const { FakeWebSocket } = await import("@/test/fake-ws");
     const sock = FakeWebSocket.latest();
     act(() => sock.open());
     const sub = sock.frames().find((f) => f.topic === "events").sub;
@@ -112,3 +111,68 @@ describe("containers list", () => {
     await waitFor(() => expect(rows(screen)).toHaveLength(3));
   });
 });
+
+const stat = (time: number, cpu: number, mem: number) => ({
+  time,
+  cpu_percent: cpu,
+  memory_used: mem,
+  memory_limit: 2_000_000_000,
+  net_rx: 0,
+  net_tx: 0,
+  block_read: 0,
+  block_write: 0,
+  pids: 3,
+});
+
+describe("containers list stats", () => {
+  it("shows live CPU and memory for running containers, one subscription each", async () => {
+    mockFetch({ ...loggedIn, "GET /api/v1/envs/local/containers": { json: LIST } });
+    const { screen } = renderApp("/envs/local/containers");
+    await waitFor(() => expect(rows(screen)).toHaveLength(2));
+    const sock = FakeWebSocket.latest();
+    act(() => sock.open());
+    await waitFor(() => expect(sock.frames().some((f) => f.topic === "stats")).toBe(true));
+    const stats = sock.frames().filter((f) => f.topic === "stats");
+    // Only the running container, and once for both its cells.
+    expect(stats).toEqual([{ op: "sub", sub: expect.any(Number), topic: "stats", env: "local", id: "c1" }]);
+    act(() => {
+      sock.receive({ sub: stats[0].sub, type: "data", data: stat(1000, 3.21, 40_000_000) });
+      sock.receive({ sub: stats[0].sub, type: "data", data: stat(2000, 12.6, 48_000_000) });
+    });
+    const web = rows(screen).find((r) => r.textContent?.includes("web"))!;
+    expect(await within(web).findByText("13%")).toBeInTheDocument();
+    expect(within(web).getByText("48.0 MB")).toBeInTheDocument();
+    expect(within(web).getByRole("img", { name: "CPU of web" }).querySelector("polyline")).toBeInTheDocument();
+    const db = rows(screen).find((r) => r.textContent?.includes("db"))!;
+    expect(within(db).getAllByText("—")).toHaveLength(2);
+  });
+
+  it("subscribes only for rows on screen", async () => {
+    const observed: { el: Element; cb: IntersectionObserverCallback }[] = [];
+    vi.stubGlobal(
+      "IntersectionObserver",
+      class {
+        cb: IntersectionObserverCallback;
+        constructor(cb: IntersectionObserverCallback) {
+          this.cb = cb;
+        }
+        observe(el: Element) {
+          observed.push({ el, cb: this.cb });
+        }
+        disconnect() {}
+      },
+    );
+    const many = Array.from({ length: 3 }, (_, i) => container({ id: `c${i}`, name: `app-${i}` }));
+    mockFetch({ ...loggedIn, "GET /api/v1/envs/local/containers": { json: many } });
+    const { screen } = renderApp("/envs/local/containers");
+    await waitFor(() => expect(rows(screen)).toHaveLength(3));
+    const sock = FakeWebSocket.latest();
+    act(() => sock.open());
+    await waitFor(() => expect(observed.length).toBeGreaterThan(0));
+    expect(sock.frames().filter((f) => f.topic === "stats")).toEqual([]);
+    const row = rows(screen).find((r) => r.textContent?.includes("app-1"))!;
+    const cell = observed.find((o) => row.contains(o.el))!;
+    act(() => cell.cb([{ isIntersecting: true } as IntersectionObserverEntry], {} as IntersectionObserver));
+    await waitFor(() => expect(sock.frames().filter((f) => f.topic === "stats").map((f) => f.id)).toEqual(["c1"]));
+  });
+});
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cd web && pnpm test src/live/stats.test.ts src/lib/lib.test.ts src/pages/containers/ContainersPage.test.tsx`
Expected: FAIL:
- `Failed to resolve import "./stats"`;
- `formatPercent is not a function`;
- the two list tests fail their first assertion: no `stats` frame, and no observed element.

- [ ] **Step 3: Write the stats store**

`web/src/live/stats.ts`:

```ts
import type { StatsSample } from "@/api/gen/types.gen";
import type { WsClient } from "./ws";

/** Samples kept per container: about two minutes at one a second. */
export const STATS_HISTORY = 120;

/** A sample with byte counters turned into rates; null where there is no earlier sample to compare. */
export interface StatsPoint extends StatsSample {
  /** Bytes per second. */
  net_rx_rate: number | null;
  net_tx_rate: number | null;
  block_read_rate: number | null;
  block_write_rate: number | null;
}

/**
 * Rates from Docker's running totals. A total that went down (the
 * container restarted, or an interface went away) counts as zero, not as
 * a negative rate.
 */
export function toPoint(s: StatsSample, prev: StatsSample | undefined): StatsPoint {
  const secs = prev ? (s.time - prev.time) / 1000 : 0;
  const rate = (now: number, then: number | undefined) =>
    prev && then !== undefined && secs > 0 ? Math.max(0, now - then) / secs : null;
  return {
    ...s,
    net_rx_rate: rate(s.net_rx, prev?.net_rx),
    net_tx_rate: rate(s.net_tx, prev?.net_tx),
    block_read_rate: rate(s.block_read, prev?.block_read),
    block_write_rate: rate(s.block_write, prev?.block_write),
  };
}

interface Entry {
  points: StatsPoint[];
  listeners: Set<() => void>;
  unsubscribe: (() => void) | null;
  teardown: ReturnType<typeof setTimeout> | null;
}

const EMPTY: StatsPoint[] = [];

/**
 * Stats for every container the tab is showing, one websocket
 * subscription per container however many components read it. When the
 * last reader goes, the subscription is kept for `graceMs` so moving from
 * the list to a container's page keeps its history.
 */
export class StatsStore {
  private readonly ws: WsClient;
  private readonly graceMs: number;
  private readonly entries = new Map<string, Entry>();

  constructor(ws: WsClient, graceMs = 5000) {
    this.ws = ws;
    this.graceMs = graceMs;
  }

  /** The history for a container: the same array until a sample arrives. */
  get(env: string, id: string): StatsPoint[] {
    return this.entries.get(key(env, id))?.points ?? EMPTY;
  }

  /** Reads a container's stats until the returned function is called. */
  subscribe(env: string, id: string, listener: () => void): () => void {
    const k = key(env, id);
    let e = this.entries.get(k);
    if (!e) {
      e = { points: EMPTY, listeners: new Set(), unsubscribe: null, teardown: null };
      this.entries.set(k, e);
    }
    const entry = e;
    if (entry.teardown) clearTimeout(entry.teardown);
    entry.teardown = null;
    entry.listeners.add(listener);
    if (!entry.unsubscribe) entry.unsubscribe = this.open(env, id, entry);
    return () => {
      if (!entry.listeners.delete(listener) || entry.listeners.size) return;
      entry.teardown = setTimeout(() => {
        entry.unsubscribe?.();
        this.entries.delete(k);
      }, this.graceMs);
    };
  }

  private open(env: string, id: string, entry: Entry) {
    const done = () => {
      // The stream ended (the container stopped) or was refused; a later
      // reader subscribes again.
      entry.unsubscribe = null;
    };
    return this.ws.subscribe(
      { topic: "stats", env, id },
      {
        onData: (d) => {
          const s = d as StatsSample;
          entry.points = [...entry.points.slice(-(STATS_HISTORY - 1)), toPoint(s, entry.points.at(-1))];
          for (const l of entry.listeners) l();
        },
        onEnd: done,
        onError: done,
      },
    ).unsubscribe;
  }
}

const key = (env: string, id: string) => `${env}\u0000${id}`;
```

`web/src/live/LiveProvider.tsx`:

```diff
--- a/web/src/live/LiveProvider.tsx
+++ b/web/src/live/LiveProvider.tsx
@@ -1,18 +1,30 @@
 import { useQuery, useQueryClient } from "@tanstack/react-query";
-import { createContext, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
+import {
+  createContext,
+  useCallback,
+  useContext,
+  useEffect,
+  useMemo,
+  useRef,
+  useState,
+  useSyncExternalStore,
+  type ReactNode,
+} from "react";
 import { ApiError, reportUnauthorized } from "@/api/client";
 import { envStatusQuery, keys } from "@/api/queries";
 import { getMe } from "@/api/gen/sdk.gen";
 import type { DockerEvent } from "@/api/gen/types.gen";
 import { EventApplier } from "./events";
+import { StatsStore, type StatsPoint } from "./stats";
 import { WsClient, wsUrl, type ConnState, type SubHandlers, type SubSpec } from "./ws";
 
 interface Live {
   ws: WsClient | null;
   state: ConnState;
+  stats: StatsStore | null;
 }
 
-const LiveContext = createContext<Live>({ ws: null, state: "connecting" });
+const LiveContext = createContext<Live>({ ws: null, state: "connecting", stats: null });
 
 /**
  * Owns the tab's websocket while a user is logged in. Environment status
@@ -34,12 +46,14 @@ export function LiveProvider({ children }: { children: ReactNode }) {
       }),
   );
 
+  const [stats] = useState(() => new StatsStore(ws));
+
   useEffect(() => {
     ws.start();
     return () => ws.close();
   }, [ws]);
 
-  const live = useMemo(() => ({ ws, state }), [ws, state]);
+  const live = useMemo(() => ({ ws, state, stats }), [ws, state, stats]);
   return <LiveContext value={live}>{children}</LiveContext>;
 }
 
@@ -73,6 +87,21 @@ export function useSubscription(spec: SubSpec | null, handlers: SubHandlers) {
   }, [ws, specKey]);
 }
 
+const NO_STATS: StatsPoint[] = [];
+
+/**
+ * A container's recent stats, live while `enabled`: one shared
+ * subscription per container across the tab (see `StatsStore`).
+ */
+export function useContainerStats(env: string, id: string, enabled: boolean): StatsPoint[] {
+  const { stats } = useLive();
+  const subscribe = useCallback(
+    (onChange: () => void) => (stats && enabled ? stats.subscribe(env, id, onChange) : () => {}),
+    [stats, env, id, enabled],
+  );
+  return useSyncExternalStore(subscribe, () => (stats && enabled ? stats.get(env, id) : NO_STATS));
+}
+
 /** Sends the user to login when a failed reconnect was a lost session. */
 async function checkSession() {
   try {
```

- [ ] **Step 4: Write the sparkline, the in-view hook and the percentage format**

`web/src/components/Sparkline.tsx`:

```tsx
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
```

`web/src/components/useInView.ts`:

```ts
import { useEffect, useState } from "react";

/**
 * Whether an element is on screen (or within 200px of it). Where the
 * browser has no IntersectionObserver, everything counts as in view.
 */
export function useInView<T extends Element>(): [(el: T | null) => void, boolean] {
  const [el, setEl] = useState<T | null>(null);
  const [inView, setInView] = useState(() => typeof IntersectionObserver === "undefined");
  useEffect(() => {
    if (!el || typeof IntersectionObserver === "undefined") return;
    const io = new IntersectionObserver(([e]) => setInView(e.isIntersecting), { rootMargin: "200px" });
    io.observe(el);
    return () => io.disconnect();
  }, [el]);
  return [setEl, inView];
}
```

`web/src/lib/format.ts`:

```diff
--- a/web/src/lib/format.ts
+++ b/web/src/lib/format.ts
@@ -31,3 +31,9 @@ export function formatAgo(unix: number, now = Date.now()): string {
 export function shortId(id: string): string {
   return id.replace(/^sha256:/, "").slice(0, 12);
 }
+
+/** A CPU percentage as `docker stats` shows it, which can pass 100 on several cores: 0.4%, 12%, 250%. */
+export function formatPercent(n: number): string {
+  if (!Number.isFinite(n) || n < 0) return "—";
+  return n < 10 ? `${n.toFixed(1)}%` : `${Math.round(n)}%`;
+}
```

- [ ] **Step 5: Add the columns, and keep a wide table inside the page**

`web/src/pages/containers/ContainersPage.tsx`:

```diff
--- a/web/src/pages/containers/ContainersPage.tsx
+++ b/web/src/pages/containers/ContainersPage.tsx
@@ -8,11 +8,14 @@ import type { ContainerSummary } from "@/api/gen/types.gen";
 import { ConfirmDialog } from "@/components/ConfirmDialog";
 import { columns, DataTable, type Column } from "@/components/DataTable";
 import { Page } from "@/components/Page";
+import { Sparkline } from "@/components/Sparkline";
 import { StateBadge } from "@/components/StateBadge";
+import { useInView } from "@/components/useInView";
 import { Button } from "@/components/ui/button";
 import { bulkMessage, runBulk } from "@/lib/bulk";
-import { formatAgo, formatBytes, shortId } from "@/lib/format";
+import { formatAgo, formatBytes, formatPercent, shortId } from "@/lib/format";
 import { portLinks } from "@/lib/ports";
+import { useContainerStats } from "@/live/LiveProvider";
 import { ACTIONS, RemoveDialog } from "./actions";
 
 const col = columns<ContainerSummary>();
@@ -49,11 +52,51 @@ function containerColumns(env: string): Column<ContainerSummary>[] {
       enableSorting: false,
       cell: (c) => <Ports container={c.row.original} />,
     }),
+    col.display({ id: "cpu", header: "CPU", cell: (c) => <Usage env={env} container={c.row.original} kind="cpu" /> }),
+    col.display({
+      id: "memory",
+      header: "Memory",
+      cell: (c) => <Usage env={env} container={c.row.original} kind="memory" />,
+    }),
     col.accessor("created", { header: "Created", cell: (c) => formatAgo(c.getValue()) }),
     col.accessor((c) => c.compose_project ?? "", { id: "compose", header: "Compose project" }),
   ];
 }
 
+/**
+ * Live CPU or memory for a running container: the latest value and a
+ * sparkline of the last minute. Only rows on screen subscribe; the CPU and
+ * memory cells of one row share a subscription.
+ */
+function Usage({ env, container, kind }: { env: string; container: ContainerSummary; kind: "cpu" | "memory" }) {
+  const [ref, inView] = useInView<HTMLDivElement>();
+  const points = useContainerStats(env, container.id, container.state === "running" && inView);
+  const last = points.at(-1);
+  if (!last) {
+    return (
+      <div ref={ref} className="text-muted-foreground">
+        —
+      </div>
+    );
+  }
+  if (kind === "cpu") {
+    const values = points.map((p) => p.cpu_percent);
+    return (
+      <div ref={ref} className="flex items-center gap-2 tabular-nums">
+        <Sparkline values={values} max={Math.max(100, ...values)} label={`CPU of ${container.name}`} />
+        <span className="w-12 text-right">{formatPercent(last.cpu_percent)}</span>
+      </div>
+    );
+  }
+  const values = points.map((p) => p.memory_used);
+  return (
+    <div ref={ref} className="flex items-center gap-2 tabular-nums" title={`Limit ${formatBytes(last.memory_limit)}`}>
+      <Sparkline values={values} max={Math.max(...values) * 1.2 || 1} label={`Memory of ${container.name}`} />
+      <span className="w-16 text-right">{formatBytes(last.memory_used)}</span>
+    </div>
+  );
+}
+
 function Ports({ container }: { container: ContainerSummary }) {
   const links = portLinks(container.ports, window.location.hostname);
   return (
```

`web/src/components/DataTable.tsx`: without this, the two new columns push the page into sideways scrolling at 1200 px. See the Global Constraints.

```diff
--- a/web/src/components/DataTable.tsx
+++ b/web/src/components/DataTable.tsx
@@ -107,7 +107,7 @@ export function DataTable<T extends RowData>(props: DataTableProps<T>) {
   const width = table.getVisibleLeafColumns().length + (props.selectable ? 1 : 0);
 
   return (
-    <div className="grid gap-3">
+    <div className="grid grid-cols-[minmax(0,1fr)] gap-3">
       <div className="flex flex-wrap items-center gap-2">
         <Input
           type="search"
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cd web && pnpm test src/live/stats.test.ts src/lib/lib.test.ts src/pages/containers/ContainersPage.test.tsx`
Expected: PASS.

- [ ] **Step 7: Run all checks**

Run: `cd web && pnpm test && pnpm typecheck && pnpm lint`
Expected: 259 passed; typecheck and lint are clean.

- [ ] **Step 8: Commit**

```bash
git add web/src/live web/src/components web/src/lib web/src/pages/containers
git commit -m "feat(web): shared stats store and live CPU and memory sparklines in the containers list"
```

---

### Task 6: The Stats tab, and the README

**Files:**
- Create: `web/src/components/Chart.tsx` and `web/src/pages/containers/stats/StatsTab.tsx`.
- Modify: `web/src/pages/containers/tabs.ts`, `web/src/pages/containers/ContainerPage.tsx` and `README.md`.
- Test: `web/src/pages/containers/stats/StatsTab.test.tsx`.

**Interfaces:**
- Consumes:
  - Task 5's `useContainerStats` and `StatsPoint`;
  - P3b's `Details`;
  - `formatBytes` and `formatPercent`.
- Produces `@/components/Chart`:
  - `interface ChartSeries { label; color (a concrete CSS colour); values: (number | null)[] }`;
  - `<Chart title times series format />`, where `times` are Unix seconds. It is a `figure` whose plot box is `role="img"` named `<title> chart`.
- Produces `StatsTab.tsx`: `<StatsTab env id running />`, loaded lazily so uPlot is in its own chunk. The current values are in a `dl` named `Current usage`.
- Produces the `stats` tab.

- [ ] **Step 1: Write the failing tests**

`web/src/pages/containers/stats/StatsTab.test.tsx`:

```tsx
import { act, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { mockFetch } from "@/test/fetch";
import { FakeWebSocket } from "@/test/fake-ws";
import { loggedIn, renderApp } from "@/test/app";
import { container, inspect } from "@/test/docker";

// uPlot draws on a canvas, which jsdom lacks; the tests read what it was given.
const { FakePlot } = vi.hoisted(() => {
  class FakePlot {
    static all: FakePlot[] = [];
    opts: { series: { label?: string }[] };
    data: unknown[];
    destroyed = false;
    constructor(opts: FakePlot["opts"], data: unknown[]) {
      this.opts = opts;
      this.data = data;
      FakePlot.all.push(this);
    }
    setData(d: unknown[]) {
      this.data = d;
    }
    setSize() {}
    destroy() {
      this.destroyed = true;
    }
    static titled(label: string) {
      return FakePlot.all.find((p) => !p.destroyed && p.opts.series[1]?.label === label)!;
    }
  }
  return { FakePlot };
});
vi.mock("uplot", () => ({ default: FakePlot }));

const WEB = inspect({ Id: "c1abcdef0123456789", Name: "/web" });
const STOPPED = inspect({ Id: "c1abcdef0123456789", Name: "/web", State: { Status: "exited", Running: false, ExitCode: 0 } });

const stat = (time: number, over: Record<string, number> = {}) => ({
  time,
  cpu_percent: 5,
  memory_used: 50_000_000,
  memory_limit: 2_000_000_000,
  net_rx: 0,
  net_tx: 0,
  block_read: 0,
  block_write: 0,
  pids: 4,
  ...over,
});

async function statsSub() {
  await waitFor(() => expect(FakeWebSocket.instances.length).toBeGreaterThan(0));
  const sock = FakeWebSocket.latest();
  if (sock.readyState !== FakeWebSocket.OPEN) act(() => sock.open());
  let sub = 0;
  await waitFor(() => {
    sub = sock.frames().find((f) => f.op === "sub" && f.topic === "stats")?.sub;
    expect(sub).toBeTruthy();
  });
  return { sock, sub };
}

describe("stats tab", () => {
  it("shows current usage and charts it as samples arrive", async () => {
    FakePlot.all = [];
    mockFetch({ ...loggedIn, "GET /api/v1/envs/local/containers/c1": { json: WEB } });
    const { screen } = renderApp("/envs/local/containers/c1?tab=stats");
    const { sock, sub } = await statsSub();
    expect(await screen.findByText("Waiting for the first sample…")).toBeInTheDocument();
    act(() => {
      sock.receive({ sub, type: "data", data: stat(1000, { net_rx: 0, block_write: 0 }) });
      sock.receive({
        sub,
        type: "data",
        data: stat(3000, { cpu_percent: 42.4, memory_used: 48_000_000, net_rx: 4000, net_tx: 1000, block_write: 2_000_000 }),
      });
    });
    const now = await screen.findByLabelText("Current usage");
    expect(within(now).getByText("42%")).toBeInTheDocument();
    expect(within(now).getByText("48.0 MB of 2.0 GB (2.4%)")).toBeInTheDocument();
    expect(within(now).getByText("2.0 KB/s in, 500 B/s out")).toBeInTheDocument();
    expect(within(now).getByText("0 B/s read, 1.0 MB/s written")).toBeInTheDocument();
    expect(within(now).getByText("4")).toBeInTheDocument();
    for (const title of ["CPU", "Memory", "Network", "Block I/O"]) {
      expect(screen.getByRole("img", { name: `${title} chart` })).toBeInTheDocument();
    }
    await waitFor(() => expect(FakePlot.titled("CPU").data).toEqual([[1, 3], [5, 42.4]]));
    expect(FakePlot.titled("In").data).toEqual([
      [1, 3],
      [null, 2000],
      [null, 500],
    ]);
  });

  it("keeps the history gathered on the list when opening a container", async () => {
    FakePlot.all = [];
    mockFetch({
      ...loggedIn,
      "GET /api/v1/envs/local/containers": { json: [container({ id: "c1", name: "web" })] },
      "GET /api/v1/envs/local/containers/c1": { json: WEB },
    });
    const { screen } = renderApp("/envs/local/containers");
    const { sock, sub } = await statsSub();
    act(() => sock.receive({ sub, type: "data", data: stat(1000, { cpu_percent: 7 }) }));
    expect(await screen.findByText("7.0%")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("link", { name: "web" }));
    await userEvent.click(await screen.findByRole("tab", { name: "Stats" }));
    const now = await screen.findByLabelText("Current usage");
    expect(within(now).getByText("7.0%")).toBeInTheDocument();
    expect(sock.frames().filter((f) => f.op === "sub" && f.topic === "stats")).toHaveLength(1);
  });

  it("says when the container is not running and does not subscribe", async () => {
    mockFetch({ ...loggedIn, "GET /api/v1/envs/local/containers/c1": { json: STOPPED } });
    const { screen } = renderApp("/envs/local/containers/c1?tab=stats");
    expect(await screen.findByText("The container is not running.")).toBeInTheDocument();
    const sock = FakeWebSocket.latest();
    act(() => sock.open());
    await waitFor(() => expect(sock.frames().some((f) => f.topic === "events")).toBe(true));
    expect(sock.frames().some((f) => f.topic === "stats")).toBe(false);
  });
});
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cd web && pnpm test src/pages/containers/stats`
Expected: FAIL, 3 tests:
- `expected undefined to be truthy`: no stats subscription;
- `Unable to find role="tab" and name "Stats"`;
- `Unable to find an element with the text: The container is not running.`

- [ ] **Step 3: Write the chart and the tab**

`web/src/components/Chart.tsx`:

```tsx
import uPlot from "uplot";
import "uplot/dist/uPlot.min.css";
import { useEffect, useRef } from "react";

export interface ChartSeries {
  label: string;
  /** A concrete CSS colour: the chart draws on a canvas, which cannot read CSS variables. */
  color: string;
  values: (number | null)[];
}

const HEIGHT = 160;
// Neutral enough to read on the light and the dark background.
const AXIS = "#8a8a8a";
const GRID = "rgba(138, 138, 138, 0.18)";

/**
 * A live time-series chart (uPlot). `times` are Unix seconds. The chart is
 * made once; new data is drawn into it, and it follows its box's width.
 * The y axis starts at zero.
 */
export function Chart({
  title,
  times,
  series,
  format,
}: {
  title: string;
  times: number[];
  series: ChartSeries[];
  format(v: number): string;
}) {
  const box = useRef<HTMLDivElement>(null);
  const plot = useRef<uPlot | null>(null);
  const latest = useRef({ times, series, format });
  useEffect(() => {
    latest.current = { times, series, format };
  });

  useEffect(() => {
    const el = box.current!;
    const { times, series, format } = latest.current;
    const fmt = (v: number | null) => (v == null ? "—" : format(v));
    const u = new uPlot(
      {
        width: el.clientWidth || 600,
        height: HEIGHT,
        legend: { show: true },
        cursor: { drag: { x: false, y: false } },
        scales: { x: { time: true }, y: { range: (_u, _min, max) => [0, max > 0 ? max * 1.1 : 1] } },
        axes: [
          { stroke: AXIS, grid: { stroke: GRID }, ticks: { stroke: GRID } },
          {
            stroke: AXIS,
            grid: { stroke: GRID },
            ticks: { stroke: GRID },
            size: 72,
            values: (_u, vals) => vals.map((v) => fmt(v)),
          },
        ],
        series: [
          {},
          ...series.map((s) => ({
            label: s.label,
            stroke: s.color,
            width: 1.5,
            fill: `${s.color}22`,
            value: (_u: uPlot, v: number | null) => fmt(v),
          })),
        ],
      },
      [times, ...series.map((s) => s.values)],
      el,
    );
    plot.current = u;
    const ro = new ResizeObserver(() => u.setSize({ width: el.clientWidth || 600, height: HEIGHT }));
    ro.observe(el);
    return () => {
      ro.disconnect();
      u.destroy();
      plot.current = null;
    };
  }, []);

  useEffect(() => {
    plot.current?.setData([times, ...series.map((s) => s.values)]);
  }, [times, series]);

  return (
    <figure className="grid gap-1">
      <figcaption className="text-sm font-medium">{title}</figcaption>
      <div ref={box} role="img" aria-label={`${title} chart`} className="min-w-0" />
    </figure>
  );
}
```

`web/src/pages/containers/stats/StatsTab.tsx`:

```tsx
import { useMemo } from "react";
import { Chart, type ChartSeries } from "@/components/Chart";
import { Details } from "@/components/Details";
import { formatBytes, formatPercent } from "@/lib/format";
import { useContainerStats } from "@/live/LiveProvider";
import type { StatsPoint } from "@/live/stats";

const rate = (v: number) => `${formatBytes(v)}/s`;

/** Live CPU, memory, network and block I/O for one container, over the last two minutes. */
export function StatsTab({ env, id, running }: { env: string; id: string; running: boolean }) {
  const points = useContainerStats(env, id, running);
  const charts = useMemo(() => chartData(points), [points]);
  if (!running) return <p className="text-sm text-muted-foreground">The container is not running.</p>;
  const last = points.at(-1);
  if (!last) return <p className="text-sm text-muted-foreground">Waiting for the first sample…</p>;
  const memPct = last.memory_limit > 0 ? ` (${formatPercent((last.memory_used / last.memory_limit) * 100)})` : "";
  return (
    <div className="grid gap-6">
      <Details
        label="Current usage"
        items={[
          ["CPU", formatPercent(last.cpu_percent)],
          ["Memory", `${formatBytes(last.memory_used)} of ${formatBytes(last.memory_limit)}${memPct}`],
          ["Network", `${fmtRate(last.net_rx_rate)} in, ${fmtRate(last.net_tx_rate)} out`],
          ["Block I/O", `${fmtRate(last.block_read_rate)} read, ${fmtRate(last.block_write_rate)} written`],
          ["Processes", String(last.pids)],
        ]}
      />
      <div className="grid gap-6 lg:grid-cols-2">
        <Chart title="CPU" times={charts.times} series={charts.cpu} format={formatPercent} />
        <Chart title="Memory" times={charts.times} series={charts.memory} format={formatBytes} />
        <Chart title="Network" times={charts.times} series={charts.network} format={rate} />
        <Chart title="Block I/O" times={charts.times} series={charts.block} format={rate} />
      </div>
    </div>
  );
}

const fmtRate = (v: number | null) => (v === null ? "—" : rate(v));

function chartData(points: StatsPoint[]) {
  const col = (f: (p: StatsPoint) => number | null) => points.map(f);
  const series = (label: string, color: string, values: (number | null)[]): ChartSeries => ({ label, color, values });
  return {
    times: points.map((p) => p.time / 1000),
    cpu: [series("CPU", "#3b82f6", col((p) => p.cpu_percent))],
    memory: [series("Used", "#8b5cf6", col((p) => p.memory_used))],
    network: [
      series("In", "#10b981", col((p) => p.net_rx_rate)),
      series("Out", "#f59e0b", col((p) => p.net_tx_rate)),
    ],
    block: [
      series("Read", "#06b6d4", col((p) => p.block_read_rate)),
      series("Written", "#ef4444", col((p) => p.block_write_rate)),
    ],
  };
}
```

- [ ] **Step 4: Add the tab, loaded lazily**

`web/src/pages/containers/tabs.ts`:

```diff
--- a/web/src/pages/containers/tabs.ts
+++ b/web/src/pages/containers/tabs.ts
@@ -1,3 +1,3 @@
 /** The container page's tabs, in order; the router validates `?tab=` against them. */
-export const CONTAINER_TABS = ["overview", "inspect", "env", "mounts", "networks", "logs", "terminal", "files"] as const;
+export const CONTAINER_TABS = ["overview", "inspect", "env", "mounts", "networks", "stats", "logs", "terminal", "files"] as const;
 export type ContainerTab = (typeof CONTAINER_TABS)[number];
```

`web/src/pages/containers/ContainerPage.tsx`:

```diff
--- a/web/src/pages/containers/ContainerPage.tsx
+++ b/web/src/pages/containers/ContainerPage.tsx
@@ -27,8 +27,10 @@ import type { ContainerTab } from "./tabs";
 
 const FROM = "/authed/shell/envs/$env/containers/$id";
 
-// xterm.js is large; it loads only when the Terminal tab opens.
+// xterm.js and uPlot load only when their tab opens.
 const TerminalTab = lazy(() => import("./terminal/TerminalTab").then((m) => ({ default: m.TerminalTab })));
+const StatsTab = lazy(() => import("./stats/StatsTab").then((m) => ({ default: m.StatsTab })));
+const loading = <p className="text-sm text-muted-foreground">Loading…</p>;
 
 export function ContainerPage() {
   const { env, id } = useParams({ from: FROM });
@@ -60,6 +62,7 @@ export function ContainerPage() {
           <TabsTrigger value="env">Env</TabsTrigger>
           <TabsTrigger value="mounts">Mounts</TabsTrigger>
           <TabsTrigger value="networks">Networks</TabsTrigger>
+          <TabsTrigger value="stats">Stats</TabsTrigger>
           <TabsTrigger value="logs">Logs</TabsTrigger>
           <TabsTrigger value="terminal">Terminal</TabsTrigger>
           <TabsTrigger value="files">Files</TabsTrigger>
@@ -79,11 +82,16 @@ export function ContainerPage() {
         <TabsContent value="networks">
           <Networks c={c} />
         </TabsContent>
+        <TabsContent value="stats">
+          <Suspense fallback={loading}>
+            <StatsTab env={env} id={id} running={c.State.Running === true} />
+          </Suspense>
+        </TabsContent>
         <TabsContent value="logs">
           <LogsTab env={env} id={id} name={name} running={c.State.Running === true} />
         </TabsContent>
         <TabsContent value="terminal">
-          <Suspense fallback={<p className="text-sm text-muted-foreground">Loading…</p>}>
+          <Suspense fallback={loading}>
             <TerminalTab env={env} id={id} running={c.State.Running === true} />
           </Suspense>
         </TabsContent>
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cd web && pnpm test src/pages/containers/stats`
Expected: PASS, 3 tests.

- [ ] **Step 6: Update the README**

`README.md`: the status, the new container features, the web stack and the roadmap.

```diff
--- a/README.md
+++ b/README.md
@@ -9,7 +9,7 @@ What sets it apart:
 - **Live UI.** Lists and detail pages update from Docker events, with no polling or manual refresh.
 - **Built for Compose and GitOps.** Stack authoring and git sync are planned (sub-project 3).
 
-> **Status: early development.** Sub-project 1 (core and local Docker) is in progress. The backend is complete. The web UI covers every resource page and the admin pages. Still to come: the log viewer, the terminal, live stats (P3c), end-to-end tests, the container image and CI (P3d).
+> **Status: early development.** Sub-project 1 (core and local Docker) is in progress. The backend and the web UI are complete: every resource page, the admin pages, the log viewer, the terminal and live stats. Still to come: end-to-end tests, the container image and CI (P3d).
 
 ## Contents
 
@@ -37,7 +37,11 @@ What sets it apart:
 - **Dashboard.** Container counts by state, disk usage, and host information.
 - **Containers.**
   - List: filter, bulk actions, clickable ports, and the Compose project.
-  - Detail page tabs: Overview, Inspect, Env, Mounts, Networks, and Files (read-only browsing with single-file download).
+  - List columns for live CPU and memory, with sparklines of the last minute. Only rows on screen subscribe.
+  - Detail page tabs: Overview, Inspect, Env, Mounts, Networks, Stats, Logs, Terminal, and Files (read-only browsing with single-file download).
+  - **Logs:** virtualized, keeping the newest 200k lines. Follow mode pauses when you scroll up. Text or regex search with highlighting and next/previous, stdout and stderr toggles, timestamps, wrapping, tail and time-range controls, ANSI colours, and download of the lines shown or the full log. A reconnect or a restart picks up where the log left off, without repeating lines.
+  - **Terminal:** xterm.js with resize. The shell is auto-detected (bash, then sh), or you can choose a shell or command and a user. Ctrl+Shift+C copies. A dropped connection ends the session rather than silently starting a new one. Every session is audited.
+  - **Stats:** live CPU, memory, network and block I/O charts for the last two minutes.
   - Create form, with an advanced JSON override.
   - **Recreate with changes:** if the new container fails to start, FleetOps rolls back to the old one.
   - **Duplicate:** host ports are cleared so the copy doesn't clash with the original.
@@ -52,14 +56,6 @@ What sets it apart:
 - Dark, light and system themes.
 - Values that look like secrets are masked until clicked. This covers env, inspect JSON, image env, and volume driver options.
 
-### Next (P3c)
-
-- **Log viewer.** Virtualized, with a 200k-line buffer, follow mode, regex search, stream and timestamp toggles, ANSI colours, and download.
-- **Terminal.** xterm.js with resize and shell auto-detect.
-- **Live stats.** CPU and memory sparklines in the container list, and charts on the detail page.
-
-The server already streams logs, exec and stats over the websocket. The UI for them lands in P3c.
-
 ## Quick start
 
 ### Requirements
@@ -145,7 +141,7 @@ Open <http://localhost:5173>. `make dev-server` keeps its data in `./.dev-data`,
 Design documents live in `docs/superpowers/`:
 
 - `specs/2026-09-29-fleetops-core-docker-design.md` is the sub-project 1 design and the authority for behaviour.
-- `plans/` holds the implementation plans, one per phase (P1, P2a–c, P3a–b so far).
+- `plans/` holds the implementation plans, one per phase (P1, P2a–c, P3a–c so far).
 
 ## Architecture
 
@@ -166,7 +162,7 @@ fleetops/
 - **Request pipeline.** Each request passes through: session auth → CSRF check (mutations only) → resolve the environment → `authorize(user, env, action)` → Docker operation → audit entry → response. `authorize` lives in `fleetops-api::authz`, so fine-grained RBAC (sub-project 5) can replace its body without touching handlers.
 - **Real time.** A per-environment hub shares one Docker events stream among all browser tabs. Stats streams are reference-counted per container.
 - **Backpressure.** Each websocket subscription has a bounded queue. Events and stats drop the oldest items when the queue is full. Logs drop lines and send a `gap` message. Exec applies true backpressure.
-- **Web stack.** TanStack Router (lazy routes), React Query, TanStack Table, Tailwind with shadcn/ui, and cmdk. Docker events patch the query cache directly.
+- **Web stack.** TanStack Router (lazy routes), React Query, TanStack Table and Virtual, Tailwind with shadcn/ui, cmdk, xterm.js and uPlot. Docker events patch the query cache directly. xterm.js and uPlot load only when their tab opens.
 
 ## API
 
@@ -220,6 +216,6 @@ Progress on sub-project 1:
   - P2c, OpenAPI
   - P3a, web foundation
   - P3b, resource and admin pages
-- **Remaining:**
   - P3c, logs, terminal and stats
+- **Remaining:**
   - P3d, Playwright e2e, container image and CI
```

- [ ] **Step 7: Run all checks**

Run: `cd web && pnpm test && pnpm typecheck && pnpm lint && pnpm check:api && pnpm build`
Expected:
- 262 passed; typecheck and lint are clean.
- `check:api` shows no diff.
- The build lists `StatsTab-*.js` as its own chunk.

- [ ] **Step 8: Smoke-test against Docker**

1. Run `make build`, then start the server:

   ```sh
   DOCKER_HOST=… ./target/release/fleetops --data "$(mktemp -d)" --bind 127.0.0.1:9125
   ```

2. Create the admin account.
3. Start a noisy container:

   ```sh
   docker run -d --name fleetops-smoke-p3c redis:7-alpine sh -c 'i=0; while true; do i=$((i+1)); printf "\033[32mINFO\033[0m tick %d\n" $i; [ $((i%5)) = 0 ] && echo "warn $i" >&2; sleep 0.2; done'
   ```

4. Check the container's tabs:
   - **Logs:** green `INFO`, a red edge on the `warn` lines, and the view staying at the newest line. Scrolling up unticks Follow. A `warn 1\d5` regex search moves between matches.
   - **Terminal:** Connect, then run `stty size` and `whoami`. The size matches the box, and the status line reads Connected. `exit 3` shows "The process exited with code 3."
   - **Stats:** the four charts move every second.
5. Check the containers list: it shows CPU and memory with sparklines, and the page does not scroll sideways.
6. Remove the container with `docker rm -f fleetops-smoke-p3c`.

- [ ] **Step 9: Commit**

```bash
git add README.md web/src/components/Chart.tsx web/src/pages/containers
git commit -m "feat(web): container stats tab with live charts; README status for P3c"
```
