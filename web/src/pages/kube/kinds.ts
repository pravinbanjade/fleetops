import type { KubeKind, RowHealth } from "@/api/gen/types.gen";

export const KUBE_KINDS: readonly KubeKind[] = [
  "namespaces", "nodes", "deployments", "statefulsets", "daemonsets", "jobs", "cronjobs",
  "pods", "services", "ingresses", "configmaps", "secrets", "events",
];
export const WORKLOADS: readonly KubeKind[] = ["deployments", "statefulsets", "daemonsets", "jobs", "cronjobs"];
export const CLUSTER_SCOPED: readonly KubeKind[] = ["namespaces", "nodes"];
export const SCALABLE: readonly KubeKind[] = ["deployments", "statefulsets"];
/** Kinds whose rows report health that is worth racking by: the rest are all "ok". */
export const HEALTH_KINDS: readonly KubeKind[] = [...WORKLOADS, "pods", "nodes"];
/** Kinds FleetOps lists and watches but never creates, edits or deletes. */
export const READ_ONLY: readonly KubeKind[] = ["events"];
export const RESTARTABLE: readonly KubeKind[] = ["deployments", "statefulsets", "daemonsets"];

export const KIND_LABEL: Record<KubeKind, string> = {
  namespaces: "Namespaces", nodes: "Nodes", deployments: "Deployments", statefulsets: "StatefulSets",
  daemonsets: "DaemonSets", jobs: "Jobs", cronjobs: "CronJobs", pods: "Pods", services: "Services",
  ingresses: "Ingresses", configmaps: "ConfigMaps", secrets: "Secrets", events: "Events",
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
};

/** What the status column shows, by kind. */
export const STATUS_HEADER: Partial<Record<KubeKind, string>> = { services: "Type", secrets: "Type", configmaps: "Data", events: "Type" };

export const asKind = (s: string): KubeKind | undefined => KUBE_KINDS.find((k) => k === s);

/** Where a row sits in the rack: the ones that need someone first. */
export const HEALTH_RANK: Record<RowHealth, number> = { bad: 0, warn: 1, ok: 2, unknown: 3 };
export const HEALTH_BAYS = [
  { id: "attention", label: "Needs attention" },
  { id: "changing", label: "Changing" },
  { id: "healthy", label: "Healthy" },
  { id: "other", label: "Other" },
];
