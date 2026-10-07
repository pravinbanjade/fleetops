import { useQueryClient } from "@tanstack/react-query";
import { useRef, useState } from "react";
import type { KubeFilter, KubeKind, KubeList, KubeRow, WatchEvent } from "@/api/gen/types.gen";
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

interface WatchState {
  /** Which list this state is about; any other list starts fresh. */
  key: string;
  paused: WatchPause | null;
  /** A snapshot has arrived: the cache is the cluster's, not a REST read. */
  synced: boolean;
}

/**
 * Keeps the cached list of `kind` in `ns` (all namespaces when empty) live
 * from the shared `kube-watch` topic. While `synced`, the REST list must
 * not refetch over it (a late response would bring back old rows). When
 * the watch ends, the page shows the data as paused (spec §6) until
 * `retry`; another list starts fresh. With `filter`, the cluster sends only
 * the rows it keeps (e.g. warning events), cached apart from the whole list.
 */
export function useKubeWatch(
  env: string,
  /** A curated kind, or a discovered one's `r/<group>/<version>/<resource>`. */
  kind: KubeKind | string,
  ns: string | undefined,
  onEvent?: (ev: WatchEvent) => void,
  filter?: KubeFilter,
) {
  const qc = useQueryClient();
  const id = `${kind}/${ns || "-"}${filter ? `/${filter}` : ""}`;
  const key = `${env}:${id}`;
  const [stored, setState] = useState<WatchState>({ key, paused: null, synced: false });
  const state: WatchState = stored.key === key ? stored : { key, paused: null, synced: false };
  const [attempt, setAttempt] = useState(0);
  const last = useRef(0);
  const pause = (message: string) => setState({ key, paused: { at: last.current || Date.now(), message }, synced: false });
  useSubscription(state.paused ? null : { topic: "kube-watch", env, id, ...(attempt ? { opts: { attempt } } : {}) }, {
    onData: (d) => {
      const ev = d as WatchEvent;
      last.current = Date.now();
      qc.setQueryData<KubeList>(keys.kube(env, kind, ns, filter), (old) => applyWatchEvent(old, ev));
      if (ev.type === "snapshot") setState((s) => (s.key === key && s.synced ? s : { key, paused: null, synced: true }));
      onEvent?.(ev);
    },
    onEnd: (m) => pause((m as { message?: string | null }).message ?? "the watch ended"),
    onError: (m) => pause(m.message),
  });
  return {
    paused: state.paused,
    synced: state.synced,
    retry: () => {
      setState({ key, paused: null, synced: false });
      setAttempt((n) => n + 1);
    },
  };
}
