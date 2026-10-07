import type { KubeKind, RowHealth } from "@/api/gen/types.gen";

export const KUBE_KINDS: readonly KubeKind[] = [
  "namespaces", "nodes", "deployments", "statefulsets", "daemonsets", "jobs", "cronjobs",
  "pods", "services", "ingresses", "configmaps", "secrets", "events",
  "replicasets", "horizontalpodautoscalers", "networkpolicies", "persistentvolumeclaims", "persistentvolumes",
  "storageclasses", "serviceaccounts", "roles", "rolebindings", "clusterroles", "clusterrolebindings",
];
export const WORKLOADS: readonly KubeKind[] = ["deployments", "statefulsets", "daemonsets", "jobs", "cronjobs"];
export const CLUSTER_SCOPED: readonly KubeKind[] = ["namespaces", "nodes", "persistentvolumes", "storageclasses", "clusterroles", "clusterrolebindings"];
export const SCALABLE: readonly KubeKind[] = ["deployments", "statefulsets"];
/** Kinds whose rows report health that is worth racking by: the rest are all "ok". */
export const HEALTH_KINDS: readonly KubeKind[] = [
  ...WORKLOADS, "pods", "nodes", "replicasets", "horizontalpodautoscalers", "persistentvolumeclaims", "persistentvolumes",
];
/** Kinds with no Create: nodes join by themselves, ReplicaSets belong to their Deployment. */
export const NO_CREATE: readonly KubeKind[] = ["nodes", "replicasets"];
/** Kinds FleetOps lists and watches but never creates, edits or deletes. */
export const READ_ONLY: readonly KubeKind[] = ["events"];
/** Suspended and resumed from FleetOps (Deployments and StatefulSets by scaling, CronJobs by `spec.suspend`). */
export const SUSPENDABLE: readonly KubeKind[] = ["deployments", "statefulsets", "cronjobs"];
export const RESTARTABLE: readonly KubeKind[] = ["deployments", "statefulsets", "daemonsets"];

export const KIND_LABEL: Record<KubeKind, string> = {
  namespaces: "Namespaces", nodes: "Nodes", deployments: "Deployments", statefulsets: "StatefulSets",
  daemonsets: "DaemonSets", jobs: "Jobs", cronjobs: "CronJobs", pods: "Pods", services: "Services",
  ingresses: "Ingresses", configmaps: "ConfigMaps", secrets: "Secrets", events: "Events",
  replicasets: "ReplicaSets", horizontalpodautoscalers: "HorizontalPodAutoscalers", networkpolicies: "NetworkPolicies",
  persistentvolumeclaims: "PersistentVolumeClaims", persistentvolumes: "PersistentVolumes", storageclasses: "StorageClasses",
  serviceaccounts: "ServiceAccounts", roles: "Roles", rolebindings: "RoleBindings", clusterroles: "ClusterRoles",
  clusterrolebindings: "ClusterRoleBindings",
};

/** Kind-specific columns: [key in KubeRow.columns, header]. */
export const KIND_COLUMNS: Record<KubeKind, [string, string][]> = {
  namespaces: [],
  nodes: [["roles", "Roles"], ["version", "Version"], ["internal_ip", "Internal IP"]],
  deployments: [["ready", "Ready"], ["up_to_date", "Up-to-date"], ["available", "Available"]],
  statefulsets: [["ready", "Ready"]],
  daemonsets: [["desired", "Desired"], ["current", "Current"], ["ready", "Ready"], ["available", "Available"]],
  jobs: [["completions", "Completions"]],
  cronjobs: [["schedule", "Schedule"], ["active", "Active"], ["last_schedule", "Last schedule"]],
  pods: [["ready", "Ready"], ["restarts", "Restarts"], ["node", "Node"], ["ip", "IP"]],
  services: [["cluster_ip", "Cluster IP"], ["external_ip", "External IP"], ["ports", "Ports"]],
  ingresses: [["class", "Class"], ["hosts", "Hosts"], ["address", "Address"]],
  configmaps: [["keys", "Keys"]],
  secrets: [["keys", "Keys"]],
  // The events list draws its own columns.
  events: [],
  replicasets: [["desired", "Desired"], ["ready", "Ready"], ["owner", "Owner"]],
  horizontalpodautoscalers: [["target", "Target"], ["min", "Min"], ["max", "Max"], ["current", "Current"], ["targets", "Targets"]],
  networkpolicies: [["pod_selector", "Pod selector"], ["policy_types", "Policy types"]],
  persistentvolumeclaims: [["volume", "Volume"], ["capacity", "Capacity"], ["access_modes", "Access modes"], ["storage_class", "Storage class"]],
  persistentvolumes: [["claim", "Claim"], ["capacity", "Capacity"], ["access_modes", "Access modes"], ["reclaim_policy", "Reclaim policy"], ["storage_class", "Storage class"]],
  storageclasses: [["provisioner", "Provisioner"], ["reclaim_policy", "Reclaim policy"], ["binding_mode", "Binding mode"], ["default", "Default"]],
  serviceaccounts: [["secrets", "Secrets"]],
  roles: [["rules", "Rules"]],
  rolebindings: [["role", "Role"], ["subjects", "Subjects"]],
  clusterroles: [["rules", "Rules"]],
  clusterrolebindings: [["role", "Role"], ["subjects", "Subjects"]],
};

/** What the status column shows, by kind. */
export const STATUS_HEADER: Partial<Record<KubeKind, string>> = {
  services: "Type", secrets: "Type", configmaps: "Data", events: "Type", networkpolicies: "Types", storageclasses: "Provisioner",
  serviceaccounts: "Status", roles: "Rules", clusterroles: "Rules", rolebindings: "Role", clusterrolebindings: "Role",
};

export const asKind = (s: string): KubeKind | undefined => KUBE_KINDS.find((k) => k === s);

/** Where a cell that names another object links: that object's kind and namespace. */
type Linker = (cols: Record<string, string>, ns: string | null) => { kind: KubeKind; ns: string } | undefined;
const kindOf = (s: string | undefined) => (s ? asKind(s) : undefined);
export const LINK_COLUMNS: Partial<Record<KubeKind, Record<string, Linker>>> = {
  persistentvolumeclaims: { volume: () => ({ kind: "persistentvolumes", ns: "-" }) },
  persistentvolumes: { claim: (c) => (c.claim_namespace ? { kind: "persistentvolumeclaims", ns: c.claim_namespace } : undefined) },
  replicasets: { owner: (_c, ns) => ({ kind: "deployments", ns: ns ?? "-" }) },
  horizontalpodautoscalers: {
    target: (c, ns) => {
      const k = kindOf(c.target_kind);
      return k && { kind: k, ns: ns ?? "-" };
    },
  },
  rolebindings: {
    role: (c, ns) => {
      const k = kindOf(c.role_kind);
      return k && { kind: k, ns: k === "clusterroles" ? "-" : (ns ?? "-") };
    },
  },
  clusterrolebindings: { role: () => ({ kind: "clusterroles", ns: "-" }) },
};

/** Where a row sits in the rack: the ones that need someone first. */
export const HEALTH_RANK: Record<RowHealth, number> = { bad: 0, warn: 1, ok: 2, unknown: 3, off: 4 };
export const HEALTH_BAYS = [
  { id: "attention", label: "Needs attention" },
  { id: "changing", label: "Changing" },
  { id: "healthy", label: "Healthy" },
  { id: "other", label: "Other" },
  // Turned off on purpose, last: parked, not a problem to chase.
  { id: "suspended", label: "Suspended" },
];
