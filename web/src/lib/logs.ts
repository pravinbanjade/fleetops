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
