import { useEnvAccess } from "@/api/access";
import { useQuery } from "@tanstack/react-query";
import { listSwarmAgents } from "@/api/gen/sdk.gen";
import { Link, Outlet, useLocation, useNavigate, useParams } from "@tanstack/react-router";
import { Boxes, Check, ChevronRight, ChevronsUpDown, LogOut, Menu, Monitor, Moon, Network, RefreshCw, Settings2, Sun, X } from "lucide-react";
import { useEffect, useState, type CSSProperties } from "react";
import { Dialog as DialogPrimitive } from "radix-ui";
import { accessOf } from "@/api/access";
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
import { envFamily, envHue, envInitials, findEnv, isKube, isSwarm, splitNodeView, type EnvFamily } from "@/lib/env";
import { cn } from "@/lib/utils";
import { useEnvEvents, useLive } from "@/live/LiveProvider";
import { CommandPalette, typing } from "./CommandPalette";
import { ADMIN_NAV, KUBE_SIDEBAR, NAV, NODE_NAV, SWARM_NAV, isGroup, kubeGroupOf, navHref, navOf, type NavItem } from "./nav";
import { useTheme, type Theme } from "@/theme";
import { UserAvatar } from "./UserAvatar";
import { CustomResourcesNav } from "@/pages/kube/generic/CustomResourcesNav";
import { LINK, useGroupToggles } from "./sidebar";
import { TopBarSlot } from "./topBarSlot";
import { ee, Slot, useEeAdminNav } from "@/ee-slots";

export function Shell() {
  const { env } = useParams({ strict: false });
  // The drawer is open for the page it was opened on, so any navigation closes it.
  const { pathname } = useLocation();
  const [drawerOn, setDrawerOn] = useState<string | null>(null);
  const drawer = drawerOn === pathname;
  const setDrawer = (open: boolean) => setDrawerOn(open ? pathname : null);
  const { data: envs = [] } = useQuery(envsQuery);
  const family = envFamily(findEnv(envs, env)?.kind);
  const [slot, setSlot] = useState<HTMLElement | null>(null);
  useGoKeys(env, family);
  return (
    <div data-edition={ee.id} className="min-h-svh md:grid md:grid-cols-[15rem_minmax(0,1fr)]">
      <aside className="sticky top-0 hidden h-svh border-r bg-rail md:block">
        <Rail env={env} family={family} />
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
            <Rail env={env} family={family} />
          </DialogPrimitive.Content>
        </DialogPrimitive.Portal>
      </DialogPrimitive.Root>
      <main className="min-w-0">
        <TopBar env={env} family={family} onMenu={() => setDrawer(true)} slotRef={setSlot} />
        <ConnectionBanner />
        <Slot name="shell.banner" />
        {env && (family === "docker" || family === "node") && <EnvLive env={env} />}
        <TopBarSlot.Provider value={slot}>
          <Outlet />
        </TopBarSlot.Provider>
      </main>
    </div>
  );
}

/** The navigation rail: brand, environment, pages, administration, user. */
/** The params a nav item's link needs in `env`. */
function navParams(item: NavItem, env: string) {
  return ("kind" in item ? { env, kind: item.kind } : { env }) as never;
}

/** A node's pages a grant limited to stacks cannot use: they reach the whole node. */
const NODE_WIDE: Record<string, string> = { Images: "images.view", Volumes: "volumes.view", Networks: "networks.view" };

function Rail({ env, family }: { env?: string; family: EnvFamily }) {
  const { can } = useEnvAccess(env);
  const nodeNav = NODE_NAV.filter((n) => !NODE_WIDE[n.label] || can(NODE_WIDE[n.label], null));
  return (
    <div className="relative flex h-full flex-col gap-5 overflow-y-auto p-3 pt-4">
      <EnvBand env={env} />
      <Link to="/" className="flex items-center gap-2 px-1.5 pt-1" aria-label="FleetOps home">
        <Mark />
        <span className="font-callsign text-[15px] font-semibold">FleetOps</span>
      </Link>
      <EnvSwitcher current={env} family={family} />
      {env && (
        <nav className="grid gap-0.5" aria-label="Environment">
          {family === "kube" ? (
            <KubeSidebar env={env} />
          ) : (
            (family === "swarm" ? SWARM_NAV : family === "node" ? nodeNav : NAV).map((item) => (
              <NavRow key={item.label} item={item} env={env} />
            ))
          )}
        </nav>
      )}
      <AdminNav />
      <div className="mt-auto border-t pt-3">
        <UserMenu />
      </div>
    </div>
  );
}

/** One page in the rail: icon, label, and its g-chord legend on hover. */
function NavRow({ item, env, nested }: { item: NavItem; env: string; nested?: boolean }) {
  const { to, label, icon: Icon, key } = item;
  return (
    <Link to={to} params={navParams(item, env)} activeOptions={{ exact: to === "/envs/$env" }} className={cn(LINK, nested && "h-7 pl-2.5")}>
      {!nested && <Icon className="size-4 shrink-0" strokeWidth={1.75} />}
      {label}
      {key && (
        <kbd aria-hidden className="ml-auto hidden font-mono text-[10px] text-muted-foreground/80 group-hover/link:inline">
          G {key.toUpperCase()}
        </kbd>
      )}
    </Link>
  );
}

/**
 * The Kubernetes rail: the overview, then each API group folded under its
 * name. A group opens by itself while one of its pages is shown, so the
 * rail stays short and the current page is never hidden behind a fold.
 */
function KubeSidebar({ env }: { env: string }) {
  const { pathname } = useLocation();
  const [toggles, setToggle] = useGroupToggles();
  const here = (item: NavItem) => {
    const href = navHref(item, env);
    return pathname === href || pathname.startsWith(`${href}/`);
  };
  const curated = KUBE_SIDEBAR.map((entry) => {
    if (!isGroup(entry)) return <NavRow key={entry.label} item={entry} env={env} />;
    const current = entry.items.some(here);
    const open = toggles[entry.id] ?? current;
    const { icon: Icon } = entry;
    const id = `nav-${entry.id}`;
    return (
      <div key={entry.id} className="grid gap-0.5">
        <button
          type="button"
          aria-expanded={open}
          aria-controls={id}
          onClick={() => setToggle(entry.id, !open)}
          className={cn(LINK, "w-full text-left outline-none focus-visible:ring-[3px] focus-visible:ring-ring/35", current && !open && "text-foreground [&_svg]:text-primary")}
        >
          <Icon className="size-4 shrink-0" strokeWidth={1.75} />
          {entry.label}
          {!open && <span className="ml-auto font-mono text-[10px] text-muted-foreground/80 tabular-nums">{entry.items.length}</span>}
          <ChevronRight
            aria-hidden
            className={cn("size-3.5 shrink-0 text-muted-foreground transition-transform duration-150 ease-out", open ? "ml-auto rotate-90" : "")}
            strokeWidth={1.75}
          />
        </button>
        {open && (
          <div id={id} className="mb-1 ml-[1.0625rem] grid gap-px border-l pl-1.5">
            {entry.items.map((item) => (
              <NavRow key={item.label} item={item} env={env} nested />
            ))}
          </div>
        )}
      </div>
    );
  });
  return (
    <>
      {curated}
      <CustomResourcesNav env={env} />
    </>
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
  const eeNav = useEeAdminNav();
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
      {eeNav.map(({ to, label, icon: Icon }) => (
        <Link key={to} to={to as never} className={LINK}>
          <Icon className="size-4 shrink-0" strokeWidth={1.75} />
          {label}
        </Link>
      ))}
    </nav>
  );
}

/** `g` then a page's key jumps to that page of the current environment. */
function useGoKeys(env: string | undefined, family: EnvFamily) {
  const navigate = useNavigate();
  useEffect(() => {
    if (!env) return;
    let armed = 0;
    const onKey = (e: KeyboardEvent) => {
      if (typing(e) || e.metaKey || e.ctrlKey || e.altKey) return;
      const k = e.key.toLowerCase();
      if (armed && Date.now() - armed < 1200) {
        armed = 0;
        const items = navOf(family);
        const hit = items.find((n) => n.key === k);
        if (hit) {
          e.preventDefault();
          void navigate({ to: hit.to, params: navParams(hit, env) });
        }
      } else if (k === "g") {
        armed = Date.now();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [env, family, navigate]);
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

function TopBar({
  env,
  family,
  onMenu,
  slotRef,
}: {
  env?: string;
  family: EnvFamily;
  onMenu: () => void;
  slotRef: (el: HTMLElement | null) => void;
}) {
  const { pathname } = useLocation();
  const items = navOf(family);
  const section =
    [...items].reverse().find((n) => env && pathname.startsWith(navHref(n, env))) ??
    ADMIN_NAV.find((n) => pathname.startsWith(n.to));
  const SectionIcon = section?.icon;
  const group = family === "kube" && section && "kind" in section ? kubeGroupOf(section) : undefined;
  return (
    <header className="sticky top-0 z-30 flex h-12 items-center gap-2 border-b bg-background/85 px-3 backdrop-blur-md md:px-6">
      <EnvBand env={env} />
      <Button variant="ghost" size="icon-sm" className="md:hidden" aria-label="Open navigation" onClick={onMenu}>
        <Menu />
      </Button>
      <nav aria-label="Breadcrumb" className="flex min-w-0 items-center gap-1.5 text-sm">
        {env && family === "node" ? (
          <NodeBreadcrumb env={env} />
        ) : (
          env && (
            <>
              <EnvTag name={env} size="sm" />
              <span className="truncate font-medium">{env}</span>
            </>
          )
        )}
        {env && section && (
          <span aria-hidden className="text-muted-foreground/60">
            /
          </span>
        )}
        {group && (
          <>
            <span className="hidden text-muted-foreground sm:inline">{group.label}</span>
            <span aria-hidden className="hidden text-muted-foreground/60 sm:inline">
              /
            </span>
          </>
        )}
        {section && SectionIcon && (
          <span className="flex min-w-0 items-center gap-1.5 text-muted-foreground">
            <SectionIcon className="size-3.5 shrink-0" strokeWidth={1.75} />
            <span className="truncate">{section.label}</span>
          </span>
        )}
      </nav>
      <div className="ml-auto flex items-center gap-3">
        <div ref={slotRef} className="flex items-center gap-2 empty:hidden" />
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
    <div role="status" className="flex items-center gap-2 border-b border-state-wait/40 bg-state-wait/12 px-4 py-2 text-sm md:px-6">
      <RefreshCw aria-hidden className="size-4 shrink-0 text-state-wait motion-safe:animate-spin [animation-duration:2s]" />
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
        "font-holder inline-flex shrink-0 items-center justify-center rounded-xs bg-[oklch(0.5_0.14_var(--env-h))] font-bold text-white",
        size === "sm" ? "size-5 text-[9px]" : "size-7 text-[11px]",
      )}
    >
      {envInitials(name)}
    </span>
  );
}

function EnvSwitcher({ current, family }: { current?: string; family: EnvFamily }) {
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
          {current && <EnvStatusBadge env={current} kube={family === "kube" || family === "swarm"} />}
          <ChevronsUpDown className="size-4 shrink-0 text-muted-foreground" />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="w-(--radix-dropdown-menu-trigger-width) min-w-56">
        <DropdownMenuLabel className="text-xs font-normal text-muted-foreground">Environments</DropdownMenuLabel>
        {envs.map((e) => (
          <DropdownMenuItem key={e.id} onSelect={() => void navigate({ to: "/envs/$env", params: { env: e.name } })}>
            <EnvTag name={e.name} size="sm" />
            {e.name}
            {isKube(e.kind) && <Boxes aria-label="Kubernetes" className="size-3.5 text-muted-foreground" />}
            {isSwarm(e.kind) && <Network aria-label="Docker Swarm" className="size-3.5 text-muted-foreground" />}
            {(() => {
              const a = accessOf(e);
              const badge = a.roleName ?? (a.role === "viewer" ? "Viewer" : null);
              return badge && <span className="rounded border px-1 text-[10px] text-muted-foreground">{badge}</span>;
            })()}
            {e.name === current && <Check className="ml-auto size-4 text-primary" />}
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

export function EnvStatusBadge({ env, kube = false }: { env: string; kube?: boolean }) {
  const { data } = useQuery(envStatusQuery(env, kube));
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
          <UserAvatar name={user.username} />
          <span className="truncate">{user.username}</span>
          <Settings2 aria-hidden className="ml-auto text-muted-foreground" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" side="top" className="w-56">
        <DropdownMenuLabel className="flex items-center gap-2">
          <UserAvatar name={user.username} size="sm" />
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

/** `swarm / Nodes / host` for a swarm node's view, each part a link back. */
function NodeBreadcrumb({ env }: { env: string }) {
  const view = splitNodeView(env)!;
  const { can } = useEnvAccess(view.swarm);
  const agents = useQuery({
    queryKey: ["env", view.swarm, "swarm", "agents"],
    enabled: can("swarm-nodes.view"),
    queryFn: async () => (await listSwarmAgents({ path: { env: view.swarm }, throwOnError: true })).data,
  });
  const host = agents.data?.find((n) => n.node_id === view.node)?.hostname ?? view.node;
  return (
    <>
      <EnvTag name={view.swarm} size="sm" />
      <Link to="/envs/$env" params={{ env: view.swarm }} className="truncate font-medium hover:underline">
        {view.swarm}
      </Link>
      <span aria-hidden className="text-muted-foreground/60">
        /
      </span>
      <Link to="/envs/$env/swarm/nodes" params={{ env: view.swarm }} className="text-muted-foreground hover:underline">
        Nodes
      </Link>
      <span aria-hidden className="text-muted-foreground/60">
        /
      </span>
      <span className="truncate font-medium">{host}</span>
    </>
  );
}
