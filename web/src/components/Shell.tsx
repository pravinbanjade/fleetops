import { useEnvAccess } from "@/api/access";
import { useQuery } from "@tanstack/react-query";
import { listSwarmAgents } from "@/api/gen/sdk.gen";
import { Link, Outlet, useLocation, useNavigate, useParams } from "@tanstack/react-router";
import { Menu, RefreshCw, X } from "lucide-react";
import { useEffect, useState } from "react";
import { Dialog as DialogPrimitive } from "radix-ui";
import { envsQuery } from "@/api/queries";
import { Button } from "@/components/ui/button";
import { envFamily, findEnv, splitNodeView, type EnvFamily } from "@/lib/env";
import { cn } from "@/lib/utils";
import { useEnvEvents, useLive } from "@/live/LiveProvider";
import { CommandPalette, typing } from "./CommandPalette";
import { useWalkKeys } from "./walk";
import { kubeGroupOf, navHref, navOf } from "./nav";
import { TopBarSlot } from "./topBarSlot";
import { ee, Slot, useAdminNav } from "@/ee-slots";
import { Library, EnvTile } from "./shell/Library";
import { navParams } from "./shell/EnvPages";
import { NowHost, SHEET_QUERY, useNowFrame } from "./shell/now";
import { useMedia } from "./useMedia";
import { Tape } from "./shell/Tape";

export { EnvTile } from "./shell/Library";

export function Shell() {
  const { pathname } = useLocation();
  return (
    <NowHost path={pathname}>
      <Frame />
    </NowHost>
  );
}

/**
 * The window: the library, the page and the "now playing" panel as rounded
 * panels on black with an 8px gap. With the panel open on a window under
 * 1536px the library folds to its tiles so the list keeps its room.
 */
function Frame() {
  const { env } = useParams({ strict: false });
  // The drawer is open for the page it was opened on, so any navigation closes it.
  const { pathname } = useLocation();
  const [drawerOn, setDrawerOn] = useState<string | null>(null);
  const drawer = drawerOn === pathname;
  const setDrawer = (open: boolean) => setDrawerOn(open ? pathname : null);
  const { data: envs = [] } = useQuery(envsQuery);
  const family = envFamily(findEnv(envs, env)?.kind);
  const [slot, setSlot] = useState<HTMLElement | null>(null);
  const { setSlot: setNowSlot, shown } = useNowFrame();
  const sheet = useMedia(SHEET_QUERY);
  const tight = useMedia("(max-width: 1535px)");
  const beside = shown && !sheet;
  const collapsed = beside && tight;
  useGoKeys(env, family);
  useWalkKeys();
  return (
    <div
      data-edition={ee.id}
      className={cn(
        "min-h-svh bg-background md:grid md:h-svh md:gap-2 md:p-2",
        env ? "md:grid-rows-[minmax(0,1fr)_34px]" : "md:grid-rows-[minmax(0,1fr)]",
        collapsed ? "md:grid-cols-[4.5rem_minmax(0,1fr)_24rem]" : beside ? "md:grid-cols-[15rem_minmax(0,1fr)_24rem]" : "md:grid-cols-[15rem_minmax(0,1fr)]",
      )}
    >
      <aside aria-label="Library" className="hidden min-h-0 overflow-hidden rounded-[10px] bg-card md:block">
        <Library env={env} family={family} collapsed={collapsed} />
      </aside>
      <DialogPrimitive.Root open={drawer} onOpenChange={setDrawer}>
        <DialogPrimitive.Portal>
          <DialogPrimitive.Overlay className="fixed inset-0 z-50 bg-black/70 duration-300 ease-drawer data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=closed]:duration-200 data-[state=open]:animate-in data-[state=open]:fade-in-0 md:hidden" />
          <DialogPrimitive.Content className="fixed inset-y-0 left-0 z-50 w-[min(18rem,85vw)] bg-card shadow-[0_0_64px_rgb(0_0_0/0.8)] outline-none duration-300 ease-drawer data-[state=closed]:animate-out data-[state=closed]:slide-out-to-left data-[state=closed]:duration-200 data-[state=open]:animate-in data-[state=open]:slide-in-from-left md:hidden">
            <DialogPrimitive.Title className="sr-only">Navigation</DialogPrimitive.Title>
            <DialogPrimitive.Description className="sr-only">Pages and environments</DialogPrimitive.Description>
            <DialogPrimitive.Close asChild>
              <Button variant="ghost" size="icon-sm" className="absolute top-3 right-3" aria-label="Close navigation">
                <X />
              </Button>
            </DialogPrimitive.Close>
            <Library env={env} family={family} collapsed={false} />
          </DialogPrimitive.Content>
        </DialogPrimitive.Portal>
      </DialogPrimitive.Root>
      <main className="min-w-0 bg-card md:min-h-0 md:overflow-y-auto md:rounded-[10px]">
        <TopBar env={env} family={family} onMenu={() => setDrawer(true)} slotRef={setSlot} />
        <ConnectionBanner />
        <Slot name="shell.banner" />
        {env && (family === "docker" || family === "node") && <EnvLive env={env} />}
        <TopBarSlot.Provider value={slot}>
          <Outlet />
        </TopBarSlot.Provider>
      </main>
      <aside
        ref={setNowSlot}
        aria-label="Selected"
        className={cn("min-h-0 overflow-hidden rounded-[10px] bg-card", beside ? "hidden md:block" : "hidden")}
      />
      {env && (
        <div className="min-w-0 bg-card max-md:sticky max-md:bottom-0 max-md:border-t md:col-span-full md:rounded-[10px]">
          <Tape env={env} family={family} />
        </div>
      )}
    </div>
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
  const admin = useAdminNav().flatMap((g) => g.items);
  const section =
    [...items].reverse().find((n) => env && pathname.startsWith(navHref(n, env))) ??
    admin.find((n) => pathname.startsWith(n.to));
  const SectionIcon = section?.icon;
  const group = family === "kube" && section && "kind" in section ? kubeGroupOf(section) : undefined;
  return (
    <header className="sticky top-0 z-30 flex h-14 items-center gap-2 bg-card px-3 md:px-6">
      <Button variant="ghost" size="icon-sm" className="md:hidden" aria-label="Open navigation" onClick={onMenu}>
        <Menu />
      </Button>
      <nav aria-label="Breadcrumb" className="flex min-w-0 items-center gap-1.5 text-sm">
        {env && family === "node" ? (
          <NodeBreadcrumb env={env} />
        ) : (
          env && (
            <>
              <EnvTile name={env} size="sm" />
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
    <div role="status" className="mx-3 flex items-center gap-2 rounded-md bg-state-wait/12 px-4 py-2 text-sm md:mx-6">
      <RefreshCw aria-hidden className="size-4 shrink-0 text-state-wait motion-safe:animate-spin [animation-duration:2s]" />
      Lost the live connection to the server. Reconnecting…
    </div>
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
      <EnvTile name={view.swarm} size="sm" />
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
