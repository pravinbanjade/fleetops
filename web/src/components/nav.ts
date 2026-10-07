import type { KubeKind } from "@/api/gen/types.gen";
import {
  Box,
  Boxes,
  Clock,
  Container,
  Copy,
  Cpu,
  Database,
  FileCog,
  FileStack,
  FolderTree,
  Gauge,
  Globe,
  HardDrive,
  KeyRound,
  Layers,
  Package,
  Play,
  ScrollText,
  Server,
  Users,
  Waypoints,
  type LucideIcon,
} from "lucide-react";

/** The environment-scoped pages, in sidebar order. */
export const NAV = [
  { to: "/envs/$env", label: "Dashboard", icon: Gauge, key: "d" },
  { to: "/envs/$env/containers", label: "Containers", icon: Container, key: "c" },
  { to: "/envs/$env/images", label: "Images", icon: Layers, key: "i" },
  { to: "/envs/$env/volumes", label: "Volumes", icon: HardDrive, key: "v" },
  { to: "/envs/$env/networks", label: "Networks", icon: Waypoints, key: "n" },
  { to: "/envs/$env/compose", label: "Compose", icon: FileStack, key: "p" },
] as const;

/** Administration pages, shown to admins. */
export const ADMIN_NAV = [
  { to: "/admin/users", label: "Users", icon: Users },
  { to: "/admin/envs", label: "Environments", icon: Server },
  { to: "/admin/audit", label: "Audit log", icon: ScrollText },
] as const;

/** A Kubernetes page in the sidebar. `kind` items link to that kind's list. */
type KubeLink = {
  to: "/envs/$env" | "/envs/$env/k8s/$kind" | "/envs/$env/k8s/helm";
  kind?: KubeKind;
  label: string;
  icon: LucideIcon;
  /** The g-chord key, for the pages an operator jumps to most. */
  key?: string;
};

/** A titled, collapsible run of related pages, as Lens and kubectl group them. */
export type KubeGroup = { id: string; label: string; icon: LucideIcon; items: readonly KubeLink[] };

const list = (kind: KubeKind, label: string, icon: LucideIcon, key?: string): KubeLink => ({
  to: "/envs/$env/k8s/$kind",
  kind,
  label,
  icon,
  key,
});

/**
 * A Kubernetes environment's sidebar: the overview and Helm stand alone,
 * everything else sits in the group kubectl's API would put it in.
 */
export const KUBE_SIDEBAR: readonly (KubeLink | KubeGroup)[] = [
  { to: "/envs/$env", label: "Overview", icon: Gauge, key: "d" },
  {
    id: "workloads",
    label: "Workloads",
    icon: Boxes,
    items: [
      list("pods", "Pods", Box, "o"),
      list("deployments", "Deployments", Boxes, "w"),
      list("statefulsets", "StatefulSets", Database),
      list("daemonsets", "DaemonSets", Copy),
      list("jobs", "Jobs", Play),
      list("cronjobs", "CronJobs", Clock),
    ],
  },
  {
    id: "network",
    label: "Network",
    icon: Waypoints,
    items: [list("services", "Services", Waypoints, "s"), list("ingresses", "Ingresses", Globe, "i")],
  },
  {
    id: "config",
    label: "Config",
    icon: FileCog,
    items: [list("configmaps", "ConfigMaps", FileCog, "m"), list("secrets", "Secrets", KeyRound, "x")],
  },
  { to: "/envs/$env/k8s/helm", label: "Helm releases", icon: Package, key: "h" },
  {
    id: "cluster",
    label: "Cluster",
    icon: Server,
    items: [list("nodes", "Nodes", Cpu, "e"), list("namespaces", "Namespaces", FolderTree, "n")],
  },
];

export const isGroup = (e: KubeLink | KubeGroup): e is KubeGroup => "items" in e;

/** Every Kubernetes page, in sidebar order. */
export const KUBE_NAV: readonly KubeLink[] = KUBE_SIDEBAR.flatMap((e) => (isGroup(e) ? e.items : [e]));

/** The group a Kubernetes page sits in, if any. */
export function kubeGroupOf(item: KubeLink): KubeGroup | undefined {
  return KUBE_SIDEBAR.find((e): e is KubeGroup => isGroup(e) && e.items.includes(item));
}

export type NavItem = (typeof NAV)[number] | KubeLink;

/** The href a nav item points at in `env`. */
export function navHref(item: NavItem, env: string): string {
  const base = item.to.replace("$env", encodeURIComponent(env));
  return "kind" in item && item.kind ? base.replace("$kind", item.kind) : base;
}
