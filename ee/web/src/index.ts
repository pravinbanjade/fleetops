import { createRoute, lazyRouteComponent } from "@tanstack/react-router";
import { KeyRound } from "lucide-react";
import type { EeModule } from "@/ee-contract";
import { AuditExport } from "./AuditExport";
import { LicenseBanner } from "./LicenseBanner";

/** The Enterprise UI (ee/LICENSE). `id` is the marker CI looks for in bundles. */
export const ee: EeModule = {
  id: "__FLEETOPS_EE__",
  adminRoutes: (admin) => [
    createRoute({
      getParentRoute: () => admin,
      path: "license",
      component: lazyRouteComponent(() => import("./LicensePage"), "LicensePage"),
    }),
  ],
  adminNav: [{ to: "/admin/license", label: "License", icon: KeyRound }],
  slots: {
    "shell.banner": { component: LicenseBanner },
    "audit.toolbar": { component: AuditExport, feature: "audit_export" },
  },
};
