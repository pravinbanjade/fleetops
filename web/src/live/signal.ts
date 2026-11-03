import { useSubscription } from "./LiveProvider";
import type { WsClient } from "./ws";

/** Calls `onSignal` each time the server signals `key`; returns the unsubscribe. */
export function subscribeSignal(ws: WsClient, key: string, onSignal: () => void): () => void {
  return ws.subscribe({ topic: "signal", id: key }, { onData: () => onSignal() }).unsubscribe;
}

/** `subscribeSignal` for a mounted component: a page refetches when `key` is signalled. */
export function useSignal(key: string, onSignal: () => void): void {
  useSubscription({ topic: "signal", id: key }, { onData: () => onSignal() });
}
