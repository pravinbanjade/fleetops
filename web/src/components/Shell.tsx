import { useQuery } from "@tanstack/react-query";
import { Link, Outlet, useNavigate, useParams } from "@tanstack/react-router";
import { ChevronsUpDown, LogOut, Monitor, Moon, Sun, User } from "lucide-react";
import { envsQuery, envStatusQuery } from "@/api/queries";
import { useLogout, useSession } from "@/auth/session";
import { StateBadge } from "@/components/StateBadge";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { useEnvEvents, useLive } from "@/live/LiveProvider";
import { CommandPalette } from "./CommandPalette";
import { ADMIN_NAV, NAV } from "./nav";
import { useTheme, type Theme } from "@/theme";

const LINK =
  "flex items-center gap-2 rounded-md px-2 py-1.5 text-sm hover:bg-accent data-[status=active]:bg-accent data-[status=active]:font-medium";

export function Shell() {
  const { env } = useParams({ strict: false });
  return (
    <div className="flex min-h-svh flex-col md:flex-row">
      {/* Below md the sidebar sits above the page, its links wrapping in rows. */}
      <aside className="flex shrink-0 flex-col gap-4 border-b p-3 md:w-56 md:border-r md:border-b-0">
        <Link to="/" className="px-2 text-lg font-semibold">
          FleetOps
        </Link>
        <EnvSwitcher current={env} />
        <CommandPalette env={env} />
        {env && (
          <nav className="flex flex-wrap gap-1 md:grid" aria-label="Environment">
            {NAV.map(({ to, label, icon: Icon }) => (
              <Link
                key={to}
                to={to}
                params={{ env }}
                activeOptions={{ exact: to === "/envs/$env" }}
                className={LINK}
              >
                <Icon className="size-4" />
                {label}
              </Link>
            ))}
          </nav>
        )}
        <AdminNav />
        <div className="md:mt-auto">
          <UserMenu />
        </div>
      </aside>
      <main className="min-w-0 flex-1">
        <ConnectionBanner />
        {env && <EnvLive env={env} />}
        <Outlet />
      </main>
    </div>
  );
}

function AdminNav() {
  const { user } = useSession();
  if (user.role !== "admin") return null;
  return (
    <nav className="flex flex-wrap items-center gap-1 md:grid" aria-label="Administration">
      <div className="w-full px-2 text-xs font-medium text-muted-foreground">Administration</div>
      {ADMIN_NAV.map(({ to, label, icon: Icon }) => (
        <Link key={to} to={to} className={LINK}>
          <Icon className="size-4" />
          {label}
        </Link>
      ))}
    </nav>
  );
}

function EnvLive({ env }: { env: string }) {
  useEnvEvents(env);
  return null;
}

function ConnectionBanner() {
  const { state } = useLive();
  if (state !== "reconnecting") return null;
  return (
    <div role="status" className="bg-destructive/10 px-4 py-2 text-sm text-destructive">
      Lost the live connection to the server. Reconnecting…
    </div>
  );
}

function EnvSwitcher({ current }: { current?: string }) {
  const { data: envs = [] } = useQuery(envsQuery);
  const navigate = useNavigate();
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="outline" className="justify-between">
          <span className="truncate">
            <span className="sr-only">Environment: </span>
            {current ?? "Choose environment"}
          </span>
          {current ? <EnvStatusBadge env={current} /> : <ChevronsUpDown className="size-4 opacity-50" />}
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="w-52">
        <DropdownMenuLabel>Environments</DropdownMenuLabel>
        {envs.map((e) => (
          <DropdownMenuItem key={e.id} onSelect={() => void navigate({ to: "/envs/$env", params: { env: e.name } })}>
            {e.name}
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

export function EnvStatusBadge({ env }: { env: string }) {
  const { data } = useQuery(envStatusQuery(env));
  if (!data) return null;
  if (data.state === "up") return <StateBadge state="up" />;
  return (
    <span title={data.reason}>
      <StateBadge state="down" />
      <span className="sr-only">: {data.reason}</span>
    </span>
  );
}

const THEMES: { value: Theme; label: string; icon: typeof Sun }[] = [
  { value: "light", label: "Light", icon: Sun },
  { value: "dark", label: "Dark", icon: Moon },
  { value: "system", label: "System", icon: Monitor },
];

function UserMenu() {
  const { user } = useSession();
  const { theme, setTheme } = useTheme();
  const logout = useLogout();
  const navigate = useNavigate();
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="ghost" className="w-full justify-start gap-2">
          <User className="size-4" />
          <span className="truncate">{user.username}</span>
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="w-52">
        <DropdownMenuLabel>
          {user.username} <span className="font-normal text-muted-foreground">({user.role})</span>
        </DropdownMenuLabel>
        <DropdownMenuItem onSelect={() => void navigate({ to: "/account" })}>Account</DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuLabel className="text-xs font-normal text-muted-foreground">Theme</DropdownMenuLabel>
        <DropdownMenuRadioGroup value={theme} onValueChange={(v) => setTheme(v as Theme)}>
          {THEMES.map(({ value, label, icon: Icon }) => (
            <DropdownMenuRadioItem key={value} value={value}>
              <Icon className="size-4" />
              {label}
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
        <DropdownMenuSeparator />
        <DropdownMenuItem onSelect={() => logout.mutate()}>
          <LogOut className="size-4" />
          Log out
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
