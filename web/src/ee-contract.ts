import type { AnyRoute } from "@tanstack/react-router";
import type { LucideIcon } from "lucide-react";
import type { ComponentType } from "react";
import type { Feature } from "@/api/gen/types.gen";

/** Places in the Community UI where an Enterprise build adds a component. */
export type SlotName = "audit.toolbar" | "shell.banner";

export interface EeNavItem {
  to: string;
  label: string;
  icon: LucideIcon;
  /** Hidden unless the license grants it. */
  feature?: Feature;
}

export interface EeSlot {
  component: ComponentType;
  feature?: Feature;
}

/** What an edition's UI module exports as `ee` (editions spec §5). */
export interface EeModule {
  /** "ce" in the stub; the Enterprise module's bundle marker otherwise. */
  id: string;
  /** Pages under /admin. */
  adminRoutes: (admin: AnyRoute) => AnyRoute[];
  adminNav: EeNavItem[];
  slots: Partial<Record<SlotName, EeSlot>>;
}
