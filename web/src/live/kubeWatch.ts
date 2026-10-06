import { useQueryClient } from "@tanstack/react-query";
import { useRef, useState } from "react";
import type { KubeKind, KubeList, KubeRow, WatchEvent } from "@/api/gen/types.gen";
import { keys } from "@/api/queries";
import { useSubscription } from "./LiveProvider";

const same = (r: KubeRow, ns: string | null | undefined, name: string) => (r.namespace ?? null) === (ns ?? null) && r.name === name;
const order = (a: KubeRow, b: KubeRow) => (a.namespace ?? "").localeCompare(b.namespace ?? "") || a.name.localeCompare(b.name);

/** A cached list after one watch event; deltas before any list are dropped (a snapshot follows). */
export function applyWatchEvent(list: KubeList | undefined, ev: WatchEvent): KubeList | undefined {
  switch (ev.type) {
    case "snapshot":
      return { rows: [...ev.rows].sort(order), truncated: false };
    case "resync":
      return list;
    case "added":
    case "modified": {
      if (!list) return list;
      const rows = list.rows.filter((r) => !same(r, ev.row.namespace, ev.row.name));
      rows.push(ev.row);
      return { ...list, rows: rows.sort(order) };
    }
    case "deleted":
      return list && { ...list, rows: list.rows.filter((r) => !same(r, ev.namespace, ev.name)) };
  }
}

/** Why a list stopped being live, and since when its data is from. */
export interface WatchPause {
  at: number;
  message: string;
}

/**
 * Keeps the cached list of `kind` in `ns` (all namespaces when empty) live
 * from the shared `kube-watch` topic. When the watch ends, the page shows
 * the data as paused (spec §6) until `retry`.
 */
export function useKubeWatch(env: string, kind: KubeKind, ns: string | undefined, onEvent?: (ev: WatchEvent) => void) {
  const qc = useQueryClient();
  const [paused, setPaused] = useState<WatchPause | null>(null);
  const [attempt, setAttempt] = useState(0);
  const last = useRef(0);
  const pause = (message: string) => setPaused({ at: last.current || Date.now(), message });
  useSubscription(
    paused ? null : { topic: "kube-watch", env, id: `${kind}/${ns || "-"}`, ...(attempt ? { opts: { attempt } } : {}) },
    {
      onData: (d) => {
        const ev = d as WatchEvent;
        last.current = Date.now();
        qc.setQueryData<KubeList>(keys.kube(env, kind, ns), (old) => applyWatchEvent(old, ev));
        onEvent?.(ev);
      },
      onEnd: (m) => pause((m as { message?: string | null }).message ?? "the watch ended"),
      onError: (m) => pause(m.message),
    },
  );
  return {
    paused,
    retry: () => {
      setPaused(null);
      setAttempt((n) => n + 1);
    },
  };
}
