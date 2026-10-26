import { useQuery, useQueryClient } from "@tanstack/react-query";
import type { KubeList } from "@/api/gen/types.gen";
import { containersQuery, keys, swarmNodesQuery } from "@/api/queries";
import type { EnvFamily } from "@/lib/env";

const KIND: Record<EnvFamily, string> = { docker: "Docker", node: "Swarm node", swarm: "Swarm", kube: "Kubernetes" };

const count = (n: number, one: string) => `${n} ${one}${n === 1 ? "" : "s"}`;

/** An environment's line in the library: its kind, and what it holds when that is known. */
export function envSummary(family: EnvFamily, counts: { containers?: number; nodes?: number; pods?: number }): string {
  const n =
    family === "swarm"
      ? counts.nodes !== undefined && count(counts.nodes, "node")
      : family === "kube"
        ? counts.pods !== undefined && count(counts.pods, "pod")
        : counts.containers !== undefined && count(counts.containers, "container");
  return n ? `${KIND[family]} · ${n}` : KIND[family];
}

/**
 * The library line for `env`. Only the current environment counts what it
 * holds, from the lists its pages already load (pods only when a pod list is
 * already cached): the library never opens a subscription of its own.
 */
export function useEnvSummary(env: string, family: EnvFamily, current: boolean): string {
  const qc = useQueryClient();
  const containers = useQuery({ ...containersQuery(env), enabled: current && (family === "docker" || family === "node") });
  const nodes = useQuery({ ...swarmNodesQuery(env), enabled: current && family === "swarm" });
  if (!current) return envSummary(family, {});
  const pods = family === "kube" ? qc.getQueryData<KubeList>(keys.kube(env, "pods", undefined))?.rows.length : undefined;
  return envSummary(family, { containers: containers.data?.length, nodes: nodes.data?.length, pods });
}
