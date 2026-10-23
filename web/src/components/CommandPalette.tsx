import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { Container, HardDrive, Layers, Play, RotateCw, Search, Server, Square, UserRound, Waypoints, Puzzle } from "lucide-react";
import { useEffect, useState } from "react";
import { toast } from "sonner";
import { containersQuery, envsQuery, imagesQuery, keys, networksQuery, volumesQuery, discoveryQuery } from "@/api/queries";
import { containerAction } from "@/api/gen/sdk.gen";
import type { ContainerActionName, ContainerSummary } from "@/api/gen/types.gen";
import { useSession } from "@/auth/session";
import { Button } from "@/components/ui/button";
import {
  CommandDialog,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
  CommandShortcut,
} from "@/components/ui/command";
import { errorMessage } from "@/lib/errors";
import { shortId } from "@/lib/format";
import { ACTIONS, actionsFor } from "@/pages/containers/actions";
import { ADMIN_NAV, KUBE_NAV, NAV, NODE_NAV, SWARM_NAV, kubeGroupOf, navHref } from "./nav";
import { refOf } from "@/pages/kube/generic/ref";
import { envFamily, findEnv } from "@/lib/env";

const MAC = typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.userAgent);

const ACTION_ICON: Partial<Record<ContainerActionName, typeof Play>> = { start: Play, stop: Square, restart: RotateCw };

/** Whether a key press happened in a text field, where shortcuts must not fire. */
export function typing(e: KeyboardEvent) {
  const t = e.target as HTMLElement | null;
  return !!t && (t.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(t.tagName));
}

/**
 * The sidebar's search button; it and ⌘K / Ctrl-K open the palette: jump to a page, a container, image,
 * volume or network of the current environment, or another environment,
 * or start, stop or restart a container. `/` focuses the page's filter.
 */
export function CommandPalette({ env }: { env?: string }) {
  const [open, setOpen] = useState(false);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setOpen((o) => !o);
      } else if (e.key === "/" && !typing(e) && !e.metaKey && !e.ctrlKey) {
        const filter = document.querySelector<HTMLInputElement>("[data-filter]");
        if (filter) {
          e.preventDefault();
          filter.focus();
        }
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
  return (
    <>
      <Button
        variant="outline"
        size="sm"
        className="w-9 justify-center gap-2 px-0 text-muted-foreground sm:w-56 sm:justify-between sm:px-2.5"
        onClick={() => setOpen(true)}
      >
        <span className="flex items-center gap-2">
          <Search className="size-4" />
          <span className="sr-only sm:not-sr-only">Search</span>
        </span>
        <kbd className="hidden rounded-xs border bg-muted px-1 font-mono text-[10px] text-muted-foreground sm:inline">
          {MAC ? "⌘K" : "Ctrl K"}
        </kbd>
      </Button>
      <CommandDialog open={open} onOpenChange={setOpen} title="Command palette" description="Jump to a page or resource">
        {open && <PaletteBody env={env} close={() => setOpen(false)} />}
      </CommandDialog>
    </>
  );
}

function PaletteBody({ env, close }: { env?: string; close: () => void }) {
  const navigate = useNavigate();
  const qc = useQueryClient();
  const { user } = useSession();
  const envs = useQuery(envsQuery);
  const on = !!env;
  const e = env ?? "";
  const family = envFamily(findEnv(envs.data, e)?.kind);
  const kube = on && family === "kube";
  const swarm = on && family === "swarm";
  // Kubernetes and Swarm environments have no Docker resources to search.
  const docker = on && !!envs.data && (family === "docker" || family === "node");
  const containers = useQuery({ ...containersQuery(e), enabled: docker });
  const images = useQuery({ ...imagesQuery(e), enabled: docker });
  const volumes = useQuery({ ...volumesQuery(e), enabled: docker });
  const networks = useQuery({ ...networksQuery(e), enabled: docker });
  const discovered = useQuery({ ...discoveryQuery(e), enabled: kube });

  /** Closes the palette, then navigates. */
  const go = (nav: () => Promise<void>) => {
    close();
    void nav();
  };
  const run = async (c: ContainerSummary, action: ContainerActionName) => {
    close();
    const a = ACTIONS.find((x) => x.action === action)!;
    try {
      await containerAction({ path: { env: e, id: c.id, action } });
      toast.success(`${a.done} ${c.name}`);
    } catch (err) {
      toast.error(`${a.label} ${c.name}: ${errorMessage(err)}`);
    } finally {
      void qc.invalidateQueries({ queryKey: keys.containers(e) });
    }
  };

  return (
    <>
      <CommandInput placeholder="Search pages, containers, images…" />
      <CommandList>
        <CommandEmpty>Nothing found.</CommandEmpty>
        <CommandGroup heading="Pages">
          {kube &&
            KUBE_NAV.map((n) => {
              const group = kubeGroupOf(n);
              return (
                <CommandItem key={n.label} value={`page ${group?.label ?? ""} ${n.label}`} onSelect={() => go(() => navigate({ href: navHref(n, e) }))}>
                  <n.icon />
                  {n.label}
                  {group && <span className="ml-auto text-xs text-muted-foreground">{group.label}</span>}
                </CommandItem>
              );
            })}
          {kube &&
            discovered.data?.kinds.map((k) => {
              const label = `${k.kind} (${k.api.group || "core"})`;
              return (
                <CommandItem
                  key={`${k.api.group}/${k.api.resource}`}
                  value={`page custom resource ${label}`}
                  onSelect={() => go(() => navigate({ to: "/envs/$env/k8s/r/$group/$version/$resource", params: { env: e, ...refOf(k) } }))}
                >
                  <Puzzle />
                  {label}
                </CommandItem>
              );
            })}
          {swarm &&
            SWARM_NAV.map((n) => (
              <CommandItem key={n.to} value={`page ${n.label}`} onSelect={() => go(() => navigate({ to: n.to, params: { env: e } }))}>
                <n.icon />
                {n.label}
              </CommandItem>
            ))}
          {docker &&
            (family === "node" ? NODE_NAV : NAV).map((n) => (
              <CommandItem key={n.to} value={`page ${n.label}`} onSelect={() => go(() => navigate({ to: n.to, params: { env: e } }))}>
                <n.icon />
                {n.label}
              </CommandItem>
            ))}
          {user.role === "admin" &&
            ADMIN_NAV.map((n) => (
              <CommandItem key={n.to} value={`page ${n.label}`} onSelect={() => go(() => navigate({ to: n.to }))}>
                <n.icon />
                {n.label}
              </CommandItem>
            ))}
          <CommandItem value="page Account" onSelect={() => go(() => navigate({ to: "/account" }))}>
            <UserRound />
            Account
          </CommandItem>
        </CommandGroup>
        {on && (
          <CommandGroup heading="Containers">
            {containers.data?.map((c) => (
              <CommandItem key={c.id} value={`container ${c.name} ${c.image}`} onSelect={() => go(() => navigate({ to: "/envs/$env/containers/$id", params: { env: e, id: c.id } }))}>
                <Container />
                {c.name}
                <CommandShortcut>{c.state}</CommandShortcut>
              </CommandItem>
            ))}
          </CommandGroup>
        )}
        {on && (
          <CommandGroup heading="Container actions">
            {containers.data?.flatMap((c) =>
              actionsFor(c.state)
                .filter((a) => ACTION_ICON[a])
                .map((a) => {
                  const Icon = ACTION_ICON[a]!;
                  const label = ACTIONS.find((x) => x.action === a)!.label;
                  return (
                    <CommandItem key={`${c.id}-${a}`} value={`${label} ${c.name}`} onSelect={() => void run(c, a)}>
                      <Icon />
                      {label} {c.name}
                    </CommandItem>
                  );
                }),
            )}
          </CommandGroup>
        )}
        {on && (
          <CommandGroup heading="Images">
            {images.data?.map((i) => (
              <CommandItem
                key={i.id}
                value={`image ${i.repo_tags.join(" ") || shortId(i.id)}`}
                onSelect={() => go(() => navigate({ to: "/envs/$env/images/$id", params: { env: e, id: i.id } }))}
              >
                <Layers />
                {i.repo_tags[0] ?? shortId(i.id)}
              </CommandItem>
            ))}
          </CommandGroup>
        )}
        {on && (
          <CommandGroup heading="Volumes">
            {volumes.data?.map((v) => (
              <CommandItem key={v.name} value={`volume ${v.name}`} onSelect={() => go(() => navigate({ to: "/envs/$env/volumes/$name", params: { env: e, name: v.name } }))}>
                <HardDrive />
                {v.name}
              </CommandItem>
            ))}
          </CommandGroup>
        )}
        {on && (
          <CommandGroup heading="Networks">
            {networks.data?.map((n) => (
              <CommandItem key={n.id} value={`network ${n.name}`} onSelect={() => go(() => navigate({ to: "/envs/$env/networks/$id", params: { env: e, id: n.id } }))}>
                <Waypoints />
                {n.name}
              </CommandItem>
            ))}
          </CommandGroup>
        )}
        <CommandGroup heading="Environments">
          {envs.data
            ?.filter((x) => x.name !== env)
            .map((x) => (
              <CommandItem key={x.id} value={`environment ${x.name}`} onSelect={() => go(() => navigate({ to: "/envs/$env", params: { env: x.name } }))}>
                <Server />
                Switch to {x.name}
              </CommandItem>
            ))}
        </CommandGroup>
      </CommandList>
    </>
  );
}
