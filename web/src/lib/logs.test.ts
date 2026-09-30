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
