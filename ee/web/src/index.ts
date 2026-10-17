import { createRoute, lazyRouteComponent } from "@tanstack/react-router";
import { KeyRound, UsersRound } from "lucide-react";
import type { EeModule } from "@/ee-contract";
import { AuditExport } from "./AuditExport";
import { LicenseBanner } from "./LicenseBanner";
import { EnvTeamAccess, UserTeamAccess } from "./UserTeamAccess";

/** The Enterprise UI (ee/LICENSE). `id` is the marker CI looks for in bundles. */
export const ee: EeModule = {
  id: "__FLEETOPS_EE__",
  adminRoutes: (admin) => [
    createRoute({
      getParentRoute: () => admin,
      path: "license",
      component: lazyRouteComponent(() => import("./LicensePage"), "LicensePage"),
    }),
    createRoute({
      getParentRoute: () => admin,
      path: "teams",
      component: lazyRouteComponent(() => import("./TeamsPage"), "TeamsPage"),
    }),
  ],
  // Teams shows without a license too: frozen teams stay visible and removable.
  adminNav: [
    { to: "/admin/teams", label: "Teams", icon: UsersRound },
    { to: "/admin/license", label: "License", icon: KeyRound },
  ],
  slots: {
    "shell.banner": { component: LicenseBanner },
    "audit.toolbar": { component: AuditExport, feature: "audit_export" },
    "user.access": { component: UserTeamAccess },
    "env.access": { component: EnvTeamAccess },
  },
};
