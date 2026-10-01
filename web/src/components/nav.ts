import {
  Box,
  Boxes,
  Container,
  Cpu,
  FileCog,
  FileStack,
  FolderTree,
  Gauge,
  Globe,
  HardDrive,
  KeyRound,
  Layers,
  ScrollText,
  Server,
  Users,
  Waypoints,
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

/** A Kubernetes environment's pages, in sidebar order. `kind` items link to a list. */
export const KUBE_NAV = [
  { to: "/envs/$env", label: "Overview", icon: Gauge, key: "d" },
  { to: "/envs/$env/k8s/$kind", kind: "deployments", label: "Workloads", icon: Boxes, key: "w" },
  { to: "/envs/$env/k8s/$kind", kind: "pods", label: "Pods", icon: Box, key: "o" },
  { to: "/envs/$env/k8s/$kind", kind: "services", label: "Services", icon: Waypoints, key: "s" },
  { to: "/envs/$env/k8s/$kind", kind: "ingresses", label: "Ingresses", icon: Globe, key: "i" },
  { to: "/envs/$env/k8s/$kind", kind: "configmaps", label: "ConfigMaps", icon: FileCog, key: "m" },
  { to: "/envs/$env/k8s/$kind", kind: "secrets", label: "Secrets", icon: KeyRound, key: "x" },
  { to: "/envs/$env/k8s/$kind", kind: "namespaces", label: "Namespaces", icon: FolderTree, key: "n" },
  { to: "/envs/$env/k8s/$kind", kind: "nodes", label: "Nodes", icon: Cpu, key: "e" },
] as const;

export type NavItem = (typeof NAV)[number] | (typeof KUBE_NAV)[number];

/** The href a nav item points at in `env`. */
export function navHref(item: NavItem, env: string): string {
  const base = item.to.replace("$env", encodeURIComponent(env));
  return "kind" in item ? base.replace("$kind", item.kind) : base;
}
