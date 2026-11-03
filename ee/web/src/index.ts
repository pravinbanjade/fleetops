import { createRoute, lazyRouteComponent } from "@tanstack/react-router";
import { useQuery } from "@tanstack/react-query";
import { BellRing, KeyRound, LogIn, ShieldAlert, ShieldCheck, UsersRound } from "lucide-react";
import type { EeModule } from "@/ee-contract";
import { AuditExport } from "./AuditExport";
import { BackupSchedule } from "./BackupSchedule";
import { EeBanners } from "./BackupBanner";
import { EnvSecuritySettings } from "./EnvSecuritySettings";
import { securitySearch } from "./security";
import { SecurityNav } from "./SecurityNav";
import { ImageBadge } from "./ImageBadge";
import { ImageVulnContent, ImageVulnTrigger } from "./ImageVulnTab";
import { UpgradeCard } from "./UpgradeCard";
import { useLeftovers } from "./leftovers";
import { rolesQuery } from "./roles";
import { EnvTeamAccess, UserTeamAccess } from "./UserTeamAccess";

/** The Enterprise UI (ee/LICENSE), part of every build. */
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
    createRoute({
      getParentRoute: () => admin,
      path: "notifications",
      component: lazyRouteComponent(() => import("./NotificationsPage"), "NotificationsPage"),
    }),
    createRoute({
      getParentRoute: () => admin,
      path: "security",
      component: lazyRouteComponent(() => import("./SecuritySettingsPage"), "SecuritySettingsPage"),
    }),
  ],
  shellRoutes: (shell) => [
    createRoute({
      getParentRoute: () => shell,
      path: "/security",
      validateSearch: securitySearch,
      component: lazyRouteComponent(() => import("./SecurityPage"), "SecurityPage"),
    }),
    createRoute({
      getParentRoute: () => shell,
      path: "/security/image",
      validateSearch: (s: Record<string, unknown>): { image: string } => ({ image: typeof s.image === "string" ? s.image : "" }),
      component: lazyRouteComponent(() => import("./SecurityImagePage"), "SecurityImagePage"),
    }),
  ],
  // Without a license, Teams, Roles and SSO show only when leftover data
  // exists (useLeftovers), so frozen ones stay visible and removable.
  adminNav: [
    { to: "/admin/teams", label: "Teams", icon: UsersRound, group: "people", feature: "teams" },
    { to: "/admin/roles", label: "Roles", icon: ShieldCheck, group: "people", feature: "rbac" },
    { to: "/admin/sso", label: "Single sign-on", icon: LogIn, group: "people", feature: "sso" },
    { to: "/admin/security", label: "Security scanning", icon: ShieldAlert, group: "infra", feature: "vuln_scan" },
    { to: "/admin/notifications", label: "Notifications", icon: BellRing, group: "infra", feature: "notifications" },
    { to: "/admin/license", label: "License", icon: KeyRound, group: "license" },
  ],
  slots: {
    "shell.banner": { component: EeBanners },
    "shell.upgrade": { component: UpgradeCard },
    "audit.toolbar": { component: AuditExport, feature: "audit_export" },
    "user.access": { component: UserTeamAccess },
    "env.access": { component: EnvTeamAccess },
    "admin.backup": { component: BackupSchedule, feature: "backup" },
    "env.settings": { component: EnvSecuritySettings, feature: "vuln_scan" },
    "shell.nav": { component: SecurityNav, feature: "vuln_scan" },
    "image.tabs.trigger": { component: ImageVulnTrigger, feature: "vuln_scan" },
    "image.tabs.content": { component: ImageVulnContent, feature: "vuln_scan" },
    "image.badge": { component: ImageBadge, feature: "vuln_scan" },
  },
  useLeftovers,
  // Only admin pages offer roles, so only admins ask for them.
  useRoleOptions: () => useQuery(rolesQuery).data ?? [],
};
