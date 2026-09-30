import { Boxes, Database, Layers, LayoutDashboard, Network, Rows3, ScrollText, Server, Users } from "lucide-react";

/** The environment-scoped pages, in sidebar order. */
export const NAV = [
  { to: "/envs/$env", label: "Dashboard", icon: LayoutDashboard },
  { to: "/envs/$env/containers", label: "Containers", icon: Boxes },
  { to: "/envs/$env/images", label: "Images", icon: Layers },
  { to: "/envs/$env/volumes", label: "Volumes", icon: Database },
  { to: "/envs/$env/networks", label: "Networks", icon: Network },
  { to: "/envs/$env/compose", label: "Compose", icon: Rows3 },
] as const;

/** Administration pages, shown to admins. */
export const ADMIN_NAV = [
  { to: "/admin/users", label: "Users", icon: Users },
  { to: "/admin/envs", label: "Environments", icon: Server },
  { to: "/admin/audit", label: "Audit log", icon: ScrollText },
] as const;
