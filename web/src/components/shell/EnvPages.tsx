import { Link, useLocation } from "@tanstack/react-router";
import { ChevronRight } from "lucide-react";
import { useEnvAccess } from "@/api/access";
import type { EnvFamily } from "@/lib/env";
import { cn } from "@/lib/utils";
import { CustomResourcesNav } from "@/pages/kube/generic/CustomResourcesNav";
import { KUBE_SIDEBAR, NAV, NODE_NAV, SWARM_NAV, isGroup, navHref, type NavItem } from "../nav";
import { LINK, useGroupToggles } from "../sidebar";

/** The params a nav item's link needs in `env`. */
export function navParams(item: NavItem, env: string) {
  return ("kind" in item ? { env, kind: item.kind } : { env }) as never;
}

/** A node's pages a grant limited to stacks cannot use: they reach the whole node. */
const NODE_WIDE: Record<string, string> = { Images: "images.view", Volumes: "volumes.view", Networks: "networks.view" };

/**
 * The current environment's pages, nested under its row in the library.
 * Collapsed, each page is its icon alone, named by its tooltip.
 */
export function EnvPages({ env, family, collapsed }: { env: string; family: EnvFamily; collapsed: boolean }) {
  const { can } = useEnvAccess(env);
  if (family === "kube" && !collapsed) return <KubeSidebar env={env} />;
  const items: readonly NavItem[] =
    family === "swarm"
      ? SWARM_NAV
      : family === "node"
        ? NODE_NAV.filter((n) => !NODE_WIDE[n.label] || can(NODE_WIDE[n.label], null))
        : family === "kube"
          ? KUBE_SIDEBAR.flatMap((e) => (isGroup(e) ? [] : [e]))
          : NAV;
  return (
    <>
      {items.map((item) => (
        <NavRow key={item.label} item={item} env={env} collapsed={collapsed} />
      ))}
    </>
  );
}

/** One page: icon, label, and its g-chord legend on hover. */
function NavRow({ item, env, nested, collapsed }: { item: NavItem; env: string; nested?: boolean; collapsed?: boolean }) {
  const { to, label, icon: Icon, key } = item;
  return (
    <Link
      to={to}
      params={navParams(item, env)}
      activeOptions={{ exact: to === "/envs/$env" }}
      aria-label={collapsed ? label : undefined}
      title={collapsed ? label : undefined}
      className={cn(LINK, nested && "h-7 pl-2.5", collapsed && "justify-center px-0")}
    >
      {!nested && <Icon className="size-4 shrink-0" strokeWidth={1.75} />}
      {!collapsed && label}
      {key && !collapsed && (
        <kbd aria-hidden className="ml-auto hidden font-mono text-[11px] text-meta group-hover/link:inline">
          G {key.toUpperCase()}
        </kbd>
      )}
    </Link>
  );
}

/**
 * The Kubernetes pages: the overview, then each API group folded under its
 * name. A group opens by itself while one of its pages is shown, so the
 * list stays short and the current page is never hidden behind a fold.
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
          className={cn(LINK, "w-full text-left outline-none focus-visible:ring-[3px] focus-visible:ring-ring/35", current && !open && "text-foreground")}
        >
          <Icon className="size-4 shrink-0" strokeWidth={1.75} />
          {entry.label}
          {!open && <span className="ml-auto font-mono text-[11px] text-meta tabular-nums">{entry.items.length}</span>}
          <ChevronRight
            aria-hidden
            className={cn("size-3.5 shrink-0 text-muted-foreground transition-transform duration-150 ease-out", open ? "ml-auto rotate-90" : "")}
            strokeWidth={1.75}
          />
        </button>
        {open && (
          <div id={id} className="mb-1 ml-[1.0625rem] grid gap-px pl-1.5">
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
