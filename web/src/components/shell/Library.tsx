import { useQuery } from "@tanstack/react-query";
import { Link, useNavigate } from "@tanstack/react-router";
import { LogOut, Settings2, UserRound } from "lucide-react";
import type { CSSProperties } from "react";
import { accessOf } from "@/api/access";
import type { EnvView } from "@/api/gen/types.gen";
import { envsQuery, envStatusQuery, meQuery } from "@/api/queries";
import { useLogout } from "@/auth/session";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { useAdminNav } from "@/ee-slots";
import { envFamily, envHue, envInitials, isKube, isSwarm, splitNodeView, type EnvFamily } from "@/lib/env";
import { cn } from "@/lib/utils";
import { LINK } from "../sidebar";
import { UserAvatar } from "../UserAvatar";
import { EnvPages } from "./EnvPages";
import { useEnvSummary } from "./summary";

/**
 * The library: every environment as a row with its tile and a one-line
 * summary, the current one expanded into its pages, then administration,
 * and the user at the foot. Collapsed, it is the tiles alone.
 */
export function Library({ env, family, collapsed }: { env?: string; family: EnvFamily; collapsed: boolean }) {
  const { data: envs = [] } = useQuery(envsQuery);
  // A swarm node's view lives under its swarm's row.
  const home = splitNodeView(env)?.swarm ?? env;
  return (
    <div className={cn("flex h-full flex-col gap-4 overflow-y-auto p-3", collapsed && "items-center px-2")}>
      <Link to="/" className={cn("flex items-center gap-2.5 px-1.5 pt-1", collapsed && "px-0")} aria-label="FleetOps home">
        <Mark />
        {!collapsed && <span className="text-[15px] font-semibold tracking-tight">FleetOps</span>}
      </Link>
      <div aria-label="Environments" role="region" className="grid w-full gap-1">
        {!collapsed && <div className="px-2 pb-1 text-xs text-meta">Environments</div>}
        {envs.map((e) => (
          <div key={e.id} className="grid gap-0.5">
            <EnvRow env={e} current={e.name === home} collapsed={collapsed} />
            {e.name === home && env && (
              <nav className={cn("grid gap-0.5", !collapsed && "pl-3", collapsed && "mt-1")} aria-label="Environment">
                <EnvPages env={env} family={family} collapsed={collapsed} />
              </nav>
            )}
          </div>
        ))}
      </div>
      <AdminNav collapsed={collapsed} />
      <div className="mt-auto w-full pt-2">
        <UserMenu collapsed={collapsed} />
      </div>
    </div>
  );
}

/** One environment: its tile, name, summary line and up/down dot. */
function EnvRow({ env, current, collapsed }: { env: EnvView; current: boolean; collapsed: boolean }) {
  const family = envFamily(env.kind);
  const summary = useEnvSummary(env.name, family, current);
  const a = accessOf(env);
  const role = a.roleName ?? (a.role === "viewer" ? "Viewer" : null);
  return (
    <Link
      to="/envs/$env"
      params={{ env: env.name }}
      aria-label={collapsed ? env.name : undefined}
      title={collapsed ? `${env.name} · ${summary}` : undefined}
      aria-current={current ? "true" : undefined}
      className={cn(
        "flex items-center gap-2.5 rounded-md p-1.5 text-sm text-muted-foreground press hover:bg-accent hover:text-foreground",
        current && "bg-selected text-foreground",
        collapsed && "justify-center",
      )}
    >
      <EnvTile name={env.name} size="lg" />
      {!collapsed && (
        <span className="grid min-w-0 flex-1 leading-tight">
          <span className="truncate font-medium">{env.name}</span>
          <span className="truncate text-xs text-meta" data-numeric>
            {summary}
            {role && " · "}
            {role && <span>{role}</span>}
          </span>
        </span>
      )}
      {!collapsed && <EnvStatusBadge env={env.name} kube={isKube(env.kind) || isSwarm(env.kind)} current={current} />}
    </Link>
  );
}

const TILE = { sm: "size-5 rounded-[4px] text-[9px]", md: "size-7 rounded-md text-[11px]", lg: "size-9 rounded-md text-xs" };

/** An environment's identity: a pale tile in its own hue with its initials. Identity, never data. */
export function EnvTile({ name, size = "md" }: { name: string; size?: "sm" | "md" | "lg" }) {
  return (
    <span
      aria-hidden
      style={{ "--env-h": envHue(name) } as CSSProperties}
      className={cn("inline-flex shrink-0 items-center justify-center bg-[oklch(0.84_0.09_var(--env-h))] font-bold text-black", TILE[size])}
    >
      {envInitials(name)}
    </span>
  );
}

/** The FleetOps mark: the favicon's F on the live green. */
export function Mark({ size = "size-7" }: { size?: string }) {
  return (
    <svg viewBox="0 0 32 32" className={cn(size, "shrink-0")} aria-hidden>
      <rect width="32" height="32" rx="8" className="fill-live" />
      <path d="M9 9h14v3.4H13v3.2h8.5V19H13v4H9z" className="fill-black" />
    </svg>
  );
}

function AdminNav({ collapsed }: { collapsed: boolean }) {
  // Read without `useSession`: the shell re-renders on navigation, and a
  // lost session clears the cache just before the move to login.
  const { data } = useQuery(meQuery);
  const groups = useAdminNav();
  if (data?.user.role !== "admin") return null;
  // People, then environments, then records, then the license: groups by proximity, not headings.
  return (
    <nav className="grid w-full gap-2" aria-label="Administration">
      {!collapsed && <div className="px-2 text-xs text-meta">Administration</div>}
      {groups.map(({ group, items }) => (
        <div key={group} className="grid gap-0.5">
          {items.map(({ to, label, icon: Icon }) => (
            <Link
              key={to}
              to={to as never}
              aria-label={collapsed ? label : undefined}
              title={collapsed ? label : undefined}
              className={cn(LINK, collapsed && "justify-center px-0")}
            >
              <Icon className="size-4 shrink-0" strokeWidth={1.75} />
              {!collapsed && label}
            </Link>
          ))}
        </div>
      ))}
    </nav>
  );
}

/** How often a cluster the operator is not in rechecks its status; Docker hosts push theirs. */
const ELSEWHERE_POLL_MS = 60_000;

export function EnvStatusBadge({ env, kube = false, current = true }: { env: string; kube?: boolean; current?: boolean }) {
  // Only the current environment polls at its usual rate: a row for every
  // environment must not keep every cluster busy from every page.
  const q = envStatusQuery(env, kube);
  const { data } = useQuery(current ? q : { ...q, refetchInterval: kube ? ELSEWHERE_POLL_MS : false });
  if (!data) return null;
  if (data.state === "up") return <StatusDot state="up" />;
  return (
    <span title={data.reason}>
      <StatusDot state="down" />
      <span className="sr-only">: {data.reason}</span>
    </span>
  );
}

/** The environment's up/down reading, compact enough for a library row. */
function StatusDot({ state }: { state: "up" | "down" }) {
  return (
    <span className="flex shrink-0 items-center">
      <span aria-hidden className={cn("size-2 rounded-full", state === "up" ? "bg-live" : "bg-state-fail")} />
      <span className="sr-only">{state}</span>
    </span>
  );
}

function UserMenu({ collapsed }: { collapsed: boolean }) {
  const { data } = useQuery(meQuery);
  if (!data) return null;
  return <UserMenuFor user={data.user} collapsed={collapsed} />;
}

function UserMenuFor({ user, collapsed }: { user: { username: string; role: string }; collapsed: boolean }) {
  const logout = useLogout();
  const navigate = useNavigate();
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          variant="ghost"
          aria-label={collapsed ? user.username : undefined}
          className={cn("h-11 w-full justify-start gap-2.5 rounded-md px-1.5 text-foreground", collapsed && "justify-center px-0")}
        >
          <UserAvatar name={user.username} />
          {!collapsed && <span className="truncate">{user.username}</span>}
          {!collapsed && <Settings2 aria-hidden className="ml-auto text-muted-foreground" />}
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" side="top" className="w-56">
        <DropdownMenuLabel className="flex items-center gap-2">
          <UserAvatar name={user.username} size="sm" />
          {user.username} <span className="font-normal text-muted-foreground">({user.role})</span>
        </DropdownMenuLabel>
        <DropdownMenuItem onSelect={() => void navigate({ to: "/account" })}>
          <UserRound className="size-4" />
          Account
        </DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuItem onSelect={() => logout.mutate()}>
          <LogOut className="size-4" />
          Log out
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
