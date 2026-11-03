import type { AnyRoute } from "@tanstack/react-router";
import type { LucideIcon } from "lucide-react";
import type { ComponentType } from "react";
import type { Feature } from "@/api/gen/types.gen";

/** Places in the Community UI where an Enterprise build adds a component, and the props each gets. */
export interface SlotProps {
  "audit.toolbar": object;
  "shell.banner": object;
  /** At the foot of the sidebar, above the user menu. */
  "shell.upgrade": { collapsed: boolean };
  /** Under a user's grants in the Access dialog. */
  "user.access": { userId: number };
  /** Under an environment's direct grants in its Access dialog. */
  "env.access": { env: string };
  /** Below the Backup page's download and restore sections. */
  "admin.backup": object;
  /** In the sidebar, between the environments and the Administration nav. */
  "shell.nav": { collapsed: boolean };
  /** At the end of an image page's tab list. */
  "image.tabs.trigger": object;
  /** After an image page's tab panels: the panels for the triggers above. */
  "image.tabs.content": { env: string; imageId: string };
  /** A cell of the image list's Security column, shown only when this slot is. */
  "image.badge": { env: string; imageId: string };
  /** In an environment's panel on the Environments admin page. */
  "env.settings": { env: string; kind: string };
}

export type SlotName = keyof SlotProps;

/** Where an admin page sits in the Administration nav, in this order. */
export const ADMIN_GROUPS = ["people", "infra", "records", "license"] as const;
export type AdminGroup = (typeof ADMIN_GROUPS)[number];

export interface EeNavItem {
  to: string;
  label: string;
  icon: LucideIcon;
  group: AdminGroup;
  /** Hidden unless the license grants it. */
  feature?: Feature;
}

export interface EeSlot<N extends SlotName = SlotName> {
  component: ComponentType<SlotProps[N]>;
  feature?: Feature;
}

/** A custom role the access editors can offer (Enterprise). */
export interface RoleOption {
  id: number;
  name: string;
}

/** What an edition's UI module exports as `ee` (editions spec §5). */
export interface EeModule {
  /** The module's marker. */
  id: string;
  /** Pages under /admin. */
  adminRoutes: (admin: AnyRoute) => AnyRoute[];
  /** Pages under the signed-in shell, outside any one environment. */
  shellRoutes?: (shell: AnyRoute) => AnyRoute[];
  adminNav: EeNavItem[];
  slots: { [N in SlotName]?: EeSlot<N> };
  /**
   * Enterprise data left from a lapsed license, by feature: its pages and
   * sections stay visible so it can be seen and removed. A hook.
   */
  useLeftovers?: () => Partial<Record<Feature, boolean>>;
  /** Custom roles for the role pickers; absent in Community. A hook. */
  useRoleOptions?: () => RoleOption[];
}
