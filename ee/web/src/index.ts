import { createRoute, lazyRouteComponent } from "@tanstack/react-router";
import { useQuery } from "@tanstack/react-query";
import { KeyRound, LogIn, ShieldCheck, UsersRound } from "lucide-react";
import type { EeModule } from "@/ee-contract";
import { AuditExport } from "./AuditExport";
import { BackupSchedule } from "./BackupSchedule";
import { EeBanners } from "./BackupBanner";
import { rolesQuery } from "./roles";
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
    createRoute({
      getParentRoute: () => admin,
      path: "roles",
      component: lazyRouteComponent(() => import("./RolesPage"), "RolesPage"),
    }),
    createRoute({
      getParentRoute: () => admin,
      path: "sso",
      component: lazyRouteComponent(() => import("./SsoPage"), "SsoPage"),
    }),
  ],
  // Teams, Roles and SSO show without a license too: frozen ones stay visible and removable.
  adminNav: [
    { to: "/admin/teams", label: "Teams", icon: UsersRound, group: "people" },
    { to: "/admin/roles", label: "Roles", icon: ShieldCheck, group: "people" },
    { to: "/admin/sso", label: "Single sign-on", icon: LogIn, group: "people" },
    { to: "/admin/license", label: "License", icon: KeyRound, group: "license" },
  ],
  slots: {
    "shell.banner": { component: EeBanners },
    "audit.toolbar": { component: AuditExport, feature: "audit_export" },
    "user.access": { component: UserTeamAccess },
    "env.access": { component: EnvTeamAccess },
    "admin.backup": { component: BackupSchedule },
  },
  // Only admin pages offer roles, so only admins ask for them.
  useRoleOptions: () => useQuery(rolesQuery).data ?? [],
};
