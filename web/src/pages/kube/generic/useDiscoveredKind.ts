import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef } from "react";
import { kubeDiscovery } from "@/api/gen/sdk.gen";
import { discoveryQuery } from "@/api/queries";
import { findKind, refKind, type Ref } from "./ref";

/**
 * The discovered kind `r`. A kind the cached discovery lacks (a CRD
 * installed since it was read) asks the server to refresh once, so its page
 * gets its columns and actions at once rather than a minute later.
 */
export function useDiscoveredKind(env: string, r: Ref) {
  const qc = useQueryClient();
  const discovery = useQuery(discoveryQuery(env));
  const kind = findKind(discovery.data, r);
  const tried = useRef<string | null>(null);
  const key = `${env}:${refKind(r)}`;
  useEffect(() => {
    if (!discovery.isSuccess || kind || tried.current === key) return;
    tried.current = key;
    void kubeDiscovery({ path: { env }, query: { refresh: "1" } }).then(({ data }) => {
      if (data) qc.setQueryData(discoveryQuery(env).queryKey, data);
    });
  }, [discovery.isSuccess, kind, key, env, qc]);
  return { kind, discovery };
}
