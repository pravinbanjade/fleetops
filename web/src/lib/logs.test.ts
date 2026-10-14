import type { LogLine } from "@/api/gen/types.gen";
import { parseAnsi } from "./ansi";
import {
  compileQuery,
  LogBuffer,
  LogMatches,
  logText,
  markSegments,
  matchRanges,
  splitTimestamp,
  timestampKey,
} from "./logs";

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

  it("starts over when synced with a new query and catches up with the same one", () => {
    const b = new LogBuffer();
    b.append(["error", "warn"].map(out));
    const m = new LogMatches(b);
    const err = compileQuery("error", false) as RegExp;
    expect(m.sync(err)).toEqual([0]);
    b.append([out("error again")]);
    expect(m.sync(err)).toEqual([0, 2]);
    expect(m.sync(compileQuery("warn", false) as RegExp)).toEqual([1]);
  });

  it("matches nothing without a query", () => {
    const b = new LogBuffer();
    b.append([out("error")]);
    const m = new LogMatches(b);
    m.setQuery(null);
    expect(m.seqs).toEqual([]);
  });
});

describe("markSegments", () => {
  it("marks matches across colour changes and keeps each part's style", () => {
    const segs = parseAnsi("ab\x1b[31mcd\x1b[0mef");
    expect(markSegments(segs, [[1, 3]])).toEqual([
      { text: "a", style: {}, mark: false },
      { text: "b", style: {}, mark: true },
      { text: "c", style: { fg: 1 }, mark: true },
      { text: "d", style: { fg: 1 }, mark: false },
      { text: "ef", style: {}, mark: false },
    ]);
  });

  it("marks several matches in one segment", () => {
    expect(markSegments(parseAnsi("xaxa"), [[1, 2], [3, 4]]).map((s) => [s.text, s.mark])).toEqual([
      ["x", false],
      ["a", true],
      ["x", false],
      ["a", true],
    ]);
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

describe("merged logs", () => {
  const l = (pod: string | undefined, sec: number, text: string) => ({
    stream: "stdout" as const,
    text: `2026-10-14T10:00:${String(sec).padStart(2, "0")}.000000000Z ${text}`,
    ...(pod ? { pod } : {}),
  });

  it("keeps each line's pod", () => {
    const b = new LogBuffer();
    b.append([l("web-a", 1, "x"), { stream: "meta", text: "▸ web-b started", pod: "web-b" }]);
    expect([...b.entries()].map((e) => [e.pod, e.stream])).toEqual([
      ["web-a", "stdout"],
      ["web-b", "meta"],
    ]);
  });

  it("resumes per pod: a slower pod's lines are kept and nothing is shown twice", () => {
    const b = new LogBuffer();
    b.append([l("a", 5, "a5"), l("b", 2, "b2")]);
    expect(b.resumeSince).toBe(Date.parse("2026-10-14T10:00:02Z") / 1000);
    b.resume();
    // The server replays each pod from the oldest second held.
    b.append([l("b", 2, "b2"), l("a", 3, "a3"), l("b", 3, "b3"), l("a", 5, "a5"), l("a", 6, "a6")]);
    expect([...b.entries()].map((e) => e.text)).toEqual(["a5", "b2", "b3", "a6"]);
  });

  it("downloads name each line's pod", () => {
    const b = new LogBuffer();
    b.append([l("web-a", 1, "x"), l(undefined, 2, "y")]);
    expect(logText(b.entries(), false)).toBe("[web-a] x\ny\n");
  });
});

describe("merged log resume bounds", () => {
  const at = (pod: string, hh: number, mm: number, text: string) => ({
    stream: "stdout" as const,
    text: `2026-10-14T${String(hh).padStart(2, "0")}:${String(mm).padStart(2, "0")}:00.000000000Z ${text}`,
    pod,
  });

  it("forgets a pod once it is gone", () => {
    const b = new LogBuffer();
    b.append([at("old", 9, 0, "x"), at("new", 12, 0, "y")]);
    b.append([{ stream: "meta", text: "◂ old gone", pod: "old" }]);
    expect(b.resumeSince).toBe(Date.parse("2026-10-14T12:00:00Z") / 1000);
  });

  it("does not reach back further than shortly before the newest line for a quiet pod", () => {
    const b = new LogBuffer();
    b.append([at("quiet", 9, 0, "x"), at("busy", 12, 0, "y")]);
    expect(b.resumeSince).toBe(Date.parse("2026-10-14T12:00:00Z") / 1000 - 10);
  });

  it("still resumes a lagging pod from its own last line inside that window", () => {
    const b = new LogBuffer();
    b.append([
      { stream: "stdout", text: "2026-10-14T12:00:55.000000000Z a", pod: "a" },
      { stream: "stdout", text: "2026-10-14T12:01:00.000000000Z b", pod: "b" },
    ]);
    expect(b.resumeSince).toBe(Date.parse("2026-10-14T12:00:55Z") / 1000);
  });
});
