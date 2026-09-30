import { observeElementRect, useVirtualizer } from "@tanstack/react-virtual";
import { useLayoutEffect, useRef, type CSSProperties } from "react";
import { cssColor, parseAnsi, type Style } from "@/lib/ansi";
import { markSegments, matchRanges, type LogBuffer, type LogEntry } from "@/lib/logs";
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
  /** The search to highlight, if any. */
  highlight?: RegExp | null;
  /** The seq of the current match: scrolled into view and marked. */
  current?: number | null;
}

/**
 * The log lines, virtualised: only the rows in view are in the DOM, so
 * the full 200k-line buffer scrolls smoothly. While following, it stays
 * scrolled to the newest line; scrolling up pauses following and
 * scrolling back to the bottom resumes it. While paused, the lines in
 * view stay put when old lines are dropped from the top.
 */
export function LogView({ buffer, version, timestamps, wrap, follow, onFollowChange, highlight, current }: Props) {
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

  // Heights measured while wrapping are stale once it is turned off (and the reverse).
  useLayoutEffect(() => {
    v.measure();
  }, [wrap, v]);

  useLayoutEffect(() => {
    if (follow && count > 0) v.scrollToIndex(count - 1, { align: "end" });
  }, [follow, version, count, v]);

  useLayoutEffect(() => {
    if (current != null && buffer.bySeq(current)) v.scrollToIndex(current - buffer.firstSeq, { align: "center" });
  }, [current, buffer, v]);

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
                e.seq === current && "bg-accent",
              )}
              style={{ transform: `translateY(${item.start}px)`, height: wrap ? undefined : ROW }}
            >
              <LogRow entry={e} timestamps={timestamps} highlight={highlight ?? null} />
            </div>
          );
        })}
      </div>
    </div>
  );
}

function LogRow({ entry, timestamps, highlight }: { entry: LogEntry; timestamps: boolean; highlight: RegExp | null }) {
  if (entry.stream === "gap") {
    return (
      <span className="text-muted-foreground italic">
        … {entry.dropped} lines skipped because the browser fell behind
      </span>
    );
  }
  const segments = parseAnsi(entry.text);
  const ranges = highlight ? matchRanges(segments.map((s) => s.text).join(""), highlight) : [];
  return (
    <>
      {timestamps && entry.ts && <span className="mr-3 text-muted-foreground">{shortTimestamp(entry.ts)}</span>}
      {markSegments(segments, ranges).map((s, i) =>
        s.mark ? (
          <mark key={i} style={css(s.style)} className="rounded-sm bg-yellow-300/80 text-inherit dark:bg-yellow-500/50">
            {s.text}
          </mark>
        ) : (
          <span key={i} style={css(s.style)}>
            {s.text}
          </span>
        ),
      )}
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
