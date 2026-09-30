import type { StatsSample } from "@/api/gen/types.gen";
import type { WsClient } from "./ws";

/** Samples kept per container: about two minutes at one a second. */
export const STATS_HISTORY = 120;

/**
 * How long after a stream ends, with readers still waiting, it is opened
 * again. A container that restarts quickly ends its stream at `die` while
 * the page never sees it stopped; one that really stopped loses its
 * readers first, as the page stops asking.
 */
export const STATS_RETRY_MS = 2000;

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
  retry: ReturnType<typeof setTimeout> | null;
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
      e = { points: EMPTY, listeners: new Set(), unsubscribe: null, teardown: null, retry: null };
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
        if (entry.retry) clearTimeout(entry.retry);
        entry.unsubscribe?.();
        this.entries.delete(k);
      }, this.graceMs);
    };
  }

  private open(env: string, id: string, entry: Entry) {
    // Refused (say, the container is gone): a later reader asks again.
    const refused = () => {
      entry.unsubscribe = null;
    };
    const ended = () => {
      entry.unsubscribe = null;
      entry.retry = setTimeout(() => {
        entry.retry = null;
        if (entry.listeners.size && !entry.unsubscribe) entry.unsubscribe = this.open(env, id, entry);
      }, STATS_RETRY_MS);
    };
    return this.ws.subscribe(
      { topic: "stats", env, id },
      {
        onData: (d) => {
          const s = d as StatsSample;
          entry.points = [...entry.points.slice(-(STATS_HISTORY - 1)), toPoint(s, entry.points.at(-1))];
          for (const l of entry.listeners) l();
        },
        onEnd: ended,
        onError: refused,
      },
    ).unsubscribe;
  }
}

const key = (env: string, id: string) => `${env}\u0000${id}`;
