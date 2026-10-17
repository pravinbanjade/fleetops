import type { KubeKind } from "@/api/gen/types.gen";
import type { Act } from "./Act";

/** The role-grid row a kind belongs to (the server's `perm::row_of`). */
export function rowOf(kind: KubeKind): string {
  switch (kind) {
    case "deployments":
    case "statefulsets":
    case "daemonsets":
    case "replicasets":
    case "jobs":
    case "cronjobs":
    case "horizontalpodautoscalers":
      return "workloads";
    case "pods":
    case "configmaps":
    case "secrets":
      return kind;
    case "services":
    case "ingresses":
    case "networkpolicies":
      return "networking";
    case "persistentvolumeclaims":
    case "persistentvolumes":
    case "storageclasses":
      return "storage";
    default:
      return "k8s-other";
  }
}

/** `verb` on `kind`'s row, e.g. `workloads.delete`. */
export function kubePerm(kind: KubeKind, verb: "view" | "create" | "operate" | "delete" | "shell" | "reveal"): string {
  return `${rowOf(kind)}.${verb}`;
}

/** The permission an action on `kind` needs. */
export function actPerm(kind: KubeKind, act: Act): string {
  return kubePerm(kind, act === "delete" ? "delete" : "operate");
}

/** Whether something may be done to an object in `ns` (cluster-scoped: no namespace). */
export type Can = (permission: string, ns?: string | null) => boolean;
