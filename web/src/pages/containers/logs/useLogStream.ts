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
