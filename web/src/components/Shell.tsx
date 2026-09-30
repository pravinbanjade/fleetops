import { useQuery } from "@tanstack/react-query";
import { Link, Outlet, useLocation, useNavigate, useParams } from "@tanstack/react-router";
import { Check, ChevronsUpDown, LogOut, Menu, Monitor, Moon, Settings2, Sun, X } from "lucide-react";
import { useEffect, useState, type CSSProperties } from "react";
import { Dialog as DialogPrimitive } from "radix-ui";
import { envsQuery, envStatusQuery, meQuery } from "@/api/queries";
import { useLogout } from "@/auth/session";
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
import { envHue, envInitials } from "@/lib/env";
import { cn } from "@/lib/utils";
import { useEnvEvents, useLive } from "@/live/LiveProvider";
import { CommandPalette, typing } from "./CommandPalette";
import { ADMIN_NAV, NAV } from "./nav";
import { useTheme, type Theme } from "@/theme";

const LINK =
  "group/link relative flex h-8 items-center gap-2.5 rounded-md px-2 text-sm text-muted-foreground transition-colors hover:bg-accent hover:text-foreground data-[status=active]:bg-cord-soft data-[status=active]:font-medium data-[status=active]:text-foreground [&[data-status=active]_svg]:text-primary";

export function Shell() {
  const { env } = useParams({ strict: false });
  // The drawer is open for the page it was opened on, so any navigation closes it.
  const { pathname } = useLocation();
  const [drawerOn, setDrawerOn] = useState<string | null>(null);
  const drawer = drawerOn === pathname;
  const setDrawer = (open: boolean) => setDrawerOn(open ? pathname : null);
  useGoKeys(env);
  return (
    <div className="min-h-svh md:grid md:grid-cols-[15rem_minmax(0,1fr)]">
      <aside className="sticky top-0 hidden h-svh border-r bg-rail md:block">
        <Rail env={env} />
      </aside>
      <DialogPrimitive.Root open={drawer} onOpenChange={setDrawer}>
        <DialogPrimitive.Portal>
          <DialogPrimitive.Overlay className="fixed inset-0 z-50 bg-[oklch(0.1_0.01_255/0.6)] data-[state=open]:animate-in data-[state=open]:fade-in-0 md:hidden" />
          <DialogPrimitive.Content className="fixed inset-y-0 left-0 z-50 w-[min(18rem,85vw)] border-r bg-rail shadow-2xl outline-none data-[state=open]:animate-in data-[state=open]:slide-in-from-left md:hidden">
            <DialogPrimitive.Title className="sr-only">Navigation</DialogPrimitive.Title>
            <DialogPrimitive.Description className="sr-only">Pages and environments</DialogPrimitive.Description>
            <DialogPrimitive.Close asChild>
              <Button variant="ghost" size="icon-sm" className="absolute top-3 right-3" aria-label="Close navigation">
                <X />
              </Button>
            </DialogPrimitive.Close>
            <Rail env={env} />
          </DialogPrimitive.Content>
        </DialogPrimitive.Portal>
      </DialogPrimitive.Root>
      <main className="min-w-0">
        <TopBar env={env} onMenu={() => setDrawer(true)} />
        <ConnectionBanner />
        {env && <EnvLive env={env} />}
        <Outlet />
      </main>
    </div>
  );
}

/** The navigation rail: brand, environment, pages, administration, user. */
function Rail({ env }: { env?: string }) {
  return (
    <div className="relative flex h-full flex-col gap-5 overflow-y-auto p-3 pt-4">
      <EnvBand env={env} />
      <Link to="/" className="flex items-center gap-2 px-1.5 pt-1" aria-label="FleetOps home">
        <Mark />
        <span className="font-callsign text-[15px] font-semibold">FleetOps</span>
      </Link>
      <EnvSwitcher current={env} />
      {env && (
        <nav className="grid gap-0.5" aria-label="Environment">
          {NAV.map(({ to, label, icon: Icon, key }) => (
            <Link key={to} to={to} params={{ env }} activeOptions={{ exact: to === "/envs/$env" }} className={LINK}>
              <Icon className="size-4 shrink-0" strokeWidth={1.75} />
              {label}
              <kbd
                aria-hidden
                className="ml-auto hidden font-mono text-[10px] text-muted-foreground/80 group-hover/link:inline"
              >
                G {key.toUpperCase()}
              </kbd>
            </Link>
          ))}
        </nav>
      )}
      <AdminNav />
      <div className="mt-auto border-t pt-3">
        <UserMenu />
      </div>
    </div>
  );
}

/** The FleetOps mark, drawn from the favicon's geometry in the theme's colours. */
function Mark() {
  return (
    <svg viewBox="0 0 32 32" className="size-6 shrink-0" aria-hidden>
      <rect width="32" height="32" rx="7" className="fill-foreground" />
      <path d="M8 10h16v3H11.5v3H21v3h-9.5v5H8z" className="fill-background" />
      <rect x="21" y="21" width="4" height="4" rx="1" className="fill-primary" />
    </svg>
  );
}

function AdminNav() {
  // Read without `useSession`: the shell re-renders on navigation, and a
  // lost session clears the cache just before the move to login.
  const { data } = useQuery(meQuery);
  if (data?.user.role !== "admin") return null;
  return (
    <nav className="grid gap-0.5" aria-label="Administration">
      <div className="px-2 pb-1 text-xs font-medium text-muted-foreground">Administration</div>
      {ADMIN_NAV.map(({ to, label, icon: Icon }) => (
        <Link key={to} to={to} className={LINK}>
          <Icon className="size-4 shrink-0" strokeWidth={1.75} />
          {label}
        </Link>
      ))}
    </nav>
  );
}

/** `g` then a page's key jumps to that page of the current environment. */
function useGoKeys(env?: string) {
  const navigate = useNavigate();
  useEffect(() => {
    if (!env) return;
    let armed = 0;
    const onKey = (e: KeyboardEvent) => {
      if (typing(e) || e.metaKey || e.ctrlKey || e.altKey) return;
      const k = e.key.toLowerCase();
      if (armed && Date.now() - armed < 1200) {
        armed = 0;
        const hit = NAV.find((n) => n.key === k);
        if (hit) {
          e.preventDefault();
          void navigate({ to: hit.to, params: { env } });
        }
      } else if (k === "g") {
        armed = Date.now();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [env, navigate]);
}

/** Where the operator is, whether the screen is live, and the palette. */
/** The environment's identity band: a flat strip of its hue. */
function EnvBand({ env }: { env?: string }) {
  if (!env) return null;
  return (
    <span
      aria-hidden
      style={{ "--env-h": envHue(env) } as CSSProperties}
      className="absolute inset-x-0 top-0 h-[3px] bg-[oklch(0.58_0.15_var(--env-h))]"
    />
  );
}

function TopBar({ env, onMenu }: { env?: string; onMenu: () => void }) {
  const { pathname } = useLocation();
  const section =
    [...NAV]
      .reverse()
      .find((n) => env && pathname.startsWith(n.to.replace("$env", encodeURIComponent(env)))) ??
    ADMIN_NAV.find((n) => pathname.startsWith(n.to));
  const SectionIcon = section?.icon;
  return (
    <header className="sticky top-0 z-30 flex h-12 items-center gap-2 border-b bg-background/85 px-3 backdrop-blur-md md:px-6">
      <EnvBand env={env} />
      <Button variant="ghost" size="icon-sm" className="md:hidden" aria-label="Open navigation" onClick={onMenu}>
        <Menu />
      </Button>
      <nav aria-label="Breadcrumb" className="flex min-w-0 items-center gap-1.5 text-sm">
        {env && (
          <>
            <EnvTag name={env} size="sm" />
            <span className="truncate font-medium">{env}</span>
          </>
        )}
        {env && section && (
          <span aria-hidden className="text-muted-foreground/60">
            /
          </span>
        )}
        {section && SectionIcon && (
          <span className="flex min-w-0 items-center gap-1.5 text-muted-foreground">
            <SectionIcon className="size-3.5 shrink-0" strokeWidth={1.75} />
            <span className="truncate">{section.label}</span>
          </span>
        )}
      </nav>
      <div className="ml-auto flex items-center gap-3">
        <LiveState />
        <CommandPalette env={env} />
      </div>
    </header>
  );
}

/** The live channel's state, always in view. */
function LiveState() {
  const { state } = useLive();
  const [tone, text] =
    state === "reconnecting"
      ? ["bg-state-wait", "Reconnecting"]
      : state === "connecting"
        ? ["bg-muted-foreground", "Connecting"]
        : ["bg-live", "Live"];
  return (
    <span className="hidden items-center gap-1.5 text-xs text-muted-foreground sm:flex">
      <span aria-hidden className="relative flex size-2">
        {state === "open" && (
          <span className="absolute inset-0 rounded-full bg-live opacity-60 motion-safe:animate-ping [animation-duration:2.4s]" />
        )}
        <span className={cn("relative size-2 rounded-full", tone)} />
      </span>
      {text}
    </span>
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
    <div role="status" className="border-b border-destructive/30 bg-destructive/10 px-6 py-2 text-sm text-destructive">
      Lost the live connection to the server. Reconnecting…
    </div>
  );
}

/** An environment's identity: a squared tag in its own hue. */
export function EnvTag({ name, size = "md" }: { name: string; size?: "sm" | "md" }) {
  return (
    <span
      aria-hidden
      style={{ "--env-h": envHue(name) } as CSSProperties}
      className={cn(
        "font-strip inline-flex shrink-0 items-center justify-center rounded-xs bg-[oklch(0.5_0.14_var(--env-h))] font-bold text-white",
        size === "sm" ? "size-5 text-[9px]" : "size-7 text-[11px]",
      )}
    >
      {envInitials(name)}
    </span>
  );
}

function EnvSwitcher({ current }: { current?: string }) {
  const { data: envs = [] } = useQuery(envsQuery);
  const navigate = useNavigate();
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          style={current ? ({ "--env-h": envHue(current) } as CSSProperties) : undefined}
          className={cn(
            "flex h-12 w-full items-center gap-2.5 rounded-lg border bg-card px-2 text-left text-sm transition-colors outline-none hover:bg-accent focus-visible:ring-[3px] focus-visible:ring-ring/35",
            current && "bg-[oklch(0.5_0.14_var(--env-h)/0.1)] hover:bg-[oklch(0.5_0.14_var(--env-h)/0.16)]",
          )}
        >
          {current && <EnvTag name={current} />}
          <span className="grid min-w-0 flex-1 leading-tight">
            <span className="text-[11px] text-muted-foreground">Environment</span>
            <span className="truncate font-medium">
              <span className="sr-only">Environment: </span>
              {current ?? "Choose environment"}
            </span>
          </span>
          {current && <EnvStatusBadge env={current} />}
          <ChevronsUpDown className="size-4 shrink-0 text-muted-foreground" />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="w-(--radix-dropdown-menu-trigger-width) min-w-56">
        <DropdownMenuLabel className="text-xs font-normal text-muted-foreground">Environments</DropdownMenuLabel>
        {envs.map((e) => (
          <DropdownMenuItem key={e.id} onSelect={() => void navigate({ to: "/envs/$env", params: { env: e.name } })}>
            <EnvTag name={e.name} size="sm" />
            {e.name}
            {e.name === current && <Check className="ml-auto size-4 text-primary" />}
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

export function EnvStatusBadge({ env }: { env: string }) {
  const { data } = useQuery(envStatusQuery(env));
  if (!data) return null;
  if (data.state === "up") return <StatusDot state="up" />;
  return (
    <span title={data.reason}>
      <StatusDot state="down" />
      <span className="sr-only">: {data.reason}</span>
    </span>
  );
}

/** The environment's up/down reading, compact enough for the switcher. */
function StatusDot({ state }: { state: "up" | "down" }) {
  return (
    <span className="flex shrink-0 items-center">
      <span aria-hidden className={cn("size-2 rounded-full", state === "up" ? "bg-live" : "bg-state-fail")} />
      <span className="sr-only">{state}</span>
    </span>
  );
}

const THEMES: { value: Theme; label: string; icon: typeof Sun }[] = [
  { value: "light", label: "Light", icon: Sun },
  { value: "dark", label: "Dark", icon: Moon },
  { value: "system", label: "System", icon: Monitor },
];

function UserMenu() {
  const { data } = useQuery(meQuery);
  if (!data) return null;
  return <UserMenuFor user={data.user} />;
}

function UserMenuFor({ user }: { user: { username: string; role: string } }) {
  const { theme, setTheme } = useTheme();
  const logout = useLogout();
  const navigate = useNavigate();
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="ghost" className="h-10 w-full justify-start gap-2.5 px-1.5 text-foreground">
          <span
            aria-hidden
            className="flex size-7 shrink-0 items-center justify-center rounded-full border bg-card text-xs font-semibold"
          >
            {user.username.slice(0, 1).toUpperCase()}
          </span>
          <span className="truncate">{user.username}</span>
          <Settings2 aria-hidden className="ml-auto text-muted-foreground" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" side="top" className="w-56">
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
