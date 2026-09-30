import { useQuery, useQueryClient } from "@tanstack/react-query";
import { createContext, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { ApiError, reportUnauthorized } from "@/api/client";
import { envStatusQuery, keys } from "@/api/queries";
import { getMe } from "@/api/gen/sdk.gen";
import type { DockerEvent } from "@/api/gen/types.gen";
import { EventApplier } from "./events";
import { WsClient, wsUrl, type ConnState, type SubHandlers, type SubSpec } from "./ws";

interface Live {
  ws: WsClient | null;
  state: ConnState;
}

const LiveContext = createContext<Live>({ ws: null, state: "connecting" });

/**
 * Owns the tab's websocket while a user is logged in. Environment status
 * frames update the status query; after a reconnect every
 * environment-scoped query is refetched, since events may have been missed.
 */
export function LiveProvider({ children }: { children: ReactNode }) {
  const qc = useQueryClient();
  const [state, setState] = useState<ConnState>("connecting");
  const [ws] = useState(
    () =>
      new WsClient({
        url: wsUrl(),
        onState: setState,
        onEnvStatus: (env, status) => qc.setQueryData(keys.envStatus(env), status),
        onReconnect: () => void qc.invalidateQueries({ queryKey: ["env"] }),
        onUnauthorized: reportUnauthorized,
        onConnectFailed: () => void checkSession(),
      }),
  );

  useEffect(() => {
    ws.start();
    return () => ws.close();
  }, [ws]);

  const live = useMemo(() => ({ ws, state }), [ws, state]);
  return <LiveContext value={live}>{children}</LiveContext>;
}

export function useLive() {
  return useContext(LiveContext);
}

/**
 * Holds a subscription while the component is mounted and `spec` is set.
 * A new subscription is made when `spec` changes by value; the latest
 * handlers are always used.
 */
export function useSubscription(spec: SubSpec | null, handlers: SubHandlers) {
  const { ws } = useLive();
  const ref = useRef(handlers);
  useEffect(() => {
    ref.current = handlers;
  });
  const specKey = spec ? JSON.stringify(spec) : null;
  useEffect(() => {
    if (!ws || !specKey) return;
    const { unsubscribe } = ws.subscribe(JSON.parse(specKey) as SubSpec, {
      onData: (d) => ref.current.onData(d),
      onBinary: (b) => ref.current.onBinary?.(b),
      onGap: (n) => ref.current.onGap?.(n),
      onEnd: (m) => ref.current.onEnd?.(m),
      onError: (m) => ref.current.onError?.(m),
      onResubscribe: () => ref.current.onResubscribe?.(),
    });
    return unsubscribe;
  }, [ws, specKey]);
}

/** Sends the user to login when a failed reconnect was a lost session. */
async function checkSession() {
  try {
    await getMe();
  } catch (e) {
    if (e instanceof ApiError && e.status === 401) reportUnauthorized();
  }
}

/**
 * Keeps `env`'s queries live from its Docker event stream. While the
 * environment is down there is no stream (the server refuses one) and its
 * status is polled; when it comes back up, everything scoped to it is
 * refetched and the stream is subscribed again.
 */
export function useEnvEvents(env: string) {
  const qc = useQueryClient();
  const [applier] = useState(() => new EventApplier(qc));
  useEffect(() => () => applier.dispose(), [applier]);
  const { data: status } = useQuery(envStatusQuery(env));
  const down = status?.state === "down";
  const wasDown = useRef(false);
  useEffect(() => {
    if (down) {
      wasDown.current = true;
    } else if (wasDown.current) {
      wasDown.current = false;
      void qc.invalidateQueries({ queryKey: keys.env(env) });
    }
  }, [down, env, qc]);
  useSubscription(down ? null : { topic: "events", env }, {
    onData: (d) => applier.apply(env, d as DockerEvent),
    onError: (m) => {
      if (m.code === "env_unavailable") qc.setQueryData(keys.envStatus(env), { state: "down", reason: m.message });
    },
  });
}
