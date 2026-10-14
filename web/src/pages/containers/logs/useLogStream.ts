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
  /** Kubernetes only: the pod's container; absent is its first. */
  container?: string;
  /** Kubernetes only: the container's previous run; never follows. */
  previous?: boolean;
}

export const DEFAULT_QUERY: LogQuery = { tail: 1000, since: null, stdout: true, stderr: true };

export type LogState = { state: "live" } | { state: "ended" } | { state: "error"; message: string };

const LIVE: LogState = { state: "live" };

/**
 * Follows a container's log into a `LogBuffer`. Timestamps are always
 * requested: they let the viewer show or hide them without asking again,
 * and let a subscription resume without repeating lines. Changing `query`
 * starts over with a new buffer. A websocket reconnect, or the container
 * starting again (a new `startedAt`, which changes on every start even
 * when a stop was never seen), resumes from the last line held: it asks
 * for everything since then, so lines logged in between are not lost.
 */
export function useLogStream(env: string, id: string, query: LogQuery, startedAt: string | undefined) {
  const { ws } = useLive();
  const key = JSON.stringify([env, id, query]);
  const [held, setHeld] = useState(() => ({ key, buffer: new LogBuffer() }));
  if (held.key !== key) setHeld({ key, buffer: new LogBuffer() });
  const buffer = held.buffer;
  const [version, setVersion] = useState(0);
  // The state of the current subscription; a new one starts out live.
  const [status, setStatus] = useState<{ buffer: LogBuffer; startedAt?: string; state: LogState } | null>(null);
  const state = status && status.buffer === buffer && status.startedAt === startedAt ? status.state : LIVE;

  useEffect(() => {
    if (!ws) return;
    const [env, id, query] = JSON.parse(key) as [string, string, LogQuery];
    const spec = () => {
      // A no-op on a new buffer; otherwise skips what is already held.
      buffer.resume();
      const since = buffer.resumeSince;
      const opts: LogOptions = { follow: !query.previous, timestamps: true, ...query };
      if (since !== null) Object.assign(opts, { since, tail: null });
      return { topic: "logs" as const, env, id, opts };
    };
    const settle = (state: LogState) => setStatus({ buffer, startedAt, state });
    const { unsubscribe } = ws.subscribe(
      spec(),
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
        onResubscribe: spec,
      },
    );
    return unsubscribe;
  }, [ws, key, startedAt, buffer]);

  const clear = useCallback(() => {
    buffer.clear();
    setVersion(buffer.version);
  }, [buffer]);

  return { buffer, version, state, clear };
}
