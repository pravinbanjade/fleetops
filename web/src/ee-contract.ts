import type { AnyRoute } from "@tanstack/react-router";
import type { LucideIcon } from "lucide-react";
import type { ComponentType } from "react";
import type { Feature } from "@/api/gen/types.gen";

/** Places in the Community UI where an Enterprise build adds a component, and the props each gets. */
export interface SlotProps {
  "audit.toolbar": object;
  "shell.banner": object;
  /** Under a user's grants in the Access dialog. */
  "user.access": { userId: number };
  /** Under an environment's direct grants in its Access dialog. */
  "env.access": { env: string };
}

export type SlotName = keyof SlotProps;

export interface EeNavItem {
  to: string;
  label: string;
  icon: LucideIcon;
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
  /** "ce" in the stub; the Enterprise module's bundle marker otherwise. */
  id: string;
  /** Pages under /admin. */
  adminRoutes: (admin: AnyRoute) => AnyRoute[];
  adminNav: EeNavItem[];
  slots: { [N in SlotName]?: EeSlot<N> };
  /** Custom roles for the role pickers; absent in Community. A hook. */
  useRoleOptions?: () => RoleOption[];
}
