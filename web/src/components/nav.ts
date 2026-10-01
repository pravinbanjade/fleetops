import { Container, FileStack, Gauge, HardDrive, Layers, ScrollText, Server, Users, Waypoints } from "lucide-react";

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
