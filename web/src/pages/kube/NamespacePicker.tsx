import { useQuery } from "@tanstack/react-query";
import { Check, ChevronsUpDown, CornerDownLeft, FolderTree } from "lucide-react";
import { Popover } from "radix-ui";
import { useCallback, useState } from "react";
import { kubeListQuery } from "@/api/queries";
import { Command, CommandEmpty, CommandGroup, CommandInput, CommandItem, CommandList } from "@/components/ui/command";
import { cn } from "@/lib/utils";

const storageKey = (env: string) => `fleetops.ns.${env}`;

function read(env: string): string | undefined {
  try {
    return localStorage.getItem(storageKey(env)) || undefined;
  } catch {
    return undefined;
  }
}

/** The namespace a Kubernetes environment's lists show, remembered per environment. `undefined` is all. */
export function useNamespace(env: string): [string | undefined, (ns?: string) => void] {
  const [ns, setNs] = useState(() => read(env));
  const set = useCallback(
    (next?: string) => {
      setNs(next);
      try {
        if (next) localStorage.setItem(storageKey(env), next);
        else localStorage.removeItem(storageKey(env));
      } catch {
        /* storage blocked: the choice lasts this page only */
      }
    },
    [env],
  );
  return [ns, set];
}

/** A namespace as Kubernetes allows it (RFC 1123 label), so a typed one is worth asking for. */
const isNamespace = (s: string) => /^[a-z0-9]([-a-z0-9]{0,61}[a-z0-9])?$/.test(s);

/**
 * Which namespace a Kubernetes page shows: a trigger in the title row that
 * opens a searchable list, "All namespaces" pinned first. Credentials that
 * may not list namespaces still get the field: type the one they can read.
 */
export function NamespacePicker({ env, value, onChange }: { env: string; value?: string; onChange: (ns?: string) => void }) {
  const { data, isError, isPending } = useQuery(kubeListQuery(env, "namespaces"));
  const [open, setOpen] = useState(false);
  const [search, setSearch] = useState("");
  const names = (data?.rows ?? []).map((r) => r.name).sort((a, b) => a.localeCompare(b));
  const typed = search.trim().toLowerCase();
  // A name the list doesn't hold is worth offering only when nothing listed
  // matches, or the list itself is unavailable to these credentials.
  const offerTyped = typed !== "" && isNamespace(typed) && !names.includes(typed) && (isError || !names.some((n) => n.includes(typed)));
  const pick = (ns?: string) => {
    onChange(ns);
    setOpen(false);
    setSearch("");
  };
  return (
    <Popover.Root open={open} onOpenChange={(o) => (setOpen(o), o || setSearch(""))}>
      <Popover.Trigger asChild>
        <button
          type="button"
          role="combobox"
          aria-label="Namespace"
          aria-expanded={open}
          aria-haspopup="listbox"
          className="inline-flex h-8 max-w-80 min-w-0 items-center gap-2 rounded-md border border-input bg-card pr-2 pl-2.5 text-sm shadow-[0_1px_1px_oklch(0_0_0/0.04)] press outline-none hover:bg-accent focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/35 aria-expanded:bg-accent"
        >
          <FolderTree aria-hidden className="size-3.5 shrink-0 text-muted-foreground" strokeWidth={1.75} />
          <span className="text-muted-foreground max-sm:sr-only">Namespace</span>
          <span className={cn("min-w-0 truncate", value && "font-ident")}>{value ?? "All namespaces"}</span>
          <ChevronsUpDown aria-hidden className="size-3.5 shrink-0 text-muted-foreground" />
        </button>
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content
          align="end"
          sideOffset={6}
          collisionPadding={16}
          className="z-50 w-[min(18rem,calc(100vw-2rem))] origin-(--radix-popover-content-transform-origin) overflow-hidden rounded-lg border bg-popover text-popover-foreground shadow-[0_12px_32px_-8px_oklch(0_0_0/0.35),0_1px_3px_oklch(0_0_0/0.12)] ease-out data-[side=bottom]:slide-in-from-top-2 data-[side=top]:slide-in-from-bottom-2 data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=closed]:zoom-out-95 data-[state=open]:animate-in data-[state=open]:fade-in-0 data-[state=open]:zoom-in-95"
        >
          {/* cmdk names its input from `label`. */}
          <Command loop label="Search namespaces">
            <CommandInput
              placeholder={isError ? "Type a namespace…" : "Search namespaces…"}
              value={search}
              onValueChange={setSearch}
              onKeyDown={(e) => {
                // cmdk doesn't highlight an always-mounted item, so Enter on a
                // typed name that is the only option goes there directly.
                if (e.key === "Enter" && offerTyped && !"all namespaces".includes(typed)) {
                  e.preventDefault();
                  pick(typed);
                }
              }}
            />
            <CommandList aria-label="Namespaces" className="max-h-72 border-t">
              <CommandGroup>
                <CommandItem value="__all__" keywords={["all namespaces"]} onSelect={() => pick(undefined)}>
                  <Check aria-hidden className={cn("size-4", value ? "invisible" : "text-primary")} />
                  All namespaces
                  {names.length > 0 && <span className="ml-auto text-xs text-muted-foreground tabular-nums">{names.length}</span>}
                </CommandItem>
                {names.map((n) => (
                  <CommandItem key={n} value={n} onSelect={() => pick(n)} className="font-ident">
                    <Check aria-hidden className={cn("size-4", n === value ? "text-primary" : "invisible")} />
                    <span className="min-w-0 truncate">{n}</span>
                  </CommandItem>
                ))}
              </CommandGroup>
              {offerTyped && (
                // Its own always-mounted group: cmdk hides a group whose items all filter out.
                <CommandGroup forceMount className="border-t">
                  <CommandItem value={`__typed__${typed}`} forceMount onSelect={() => pick(typed)}>
                    <CornerDownLeft aria-hidden className="size-4" />
                    <span className="min-w-0 truncate">
                      Show <span className="font-ident">{typed}</span>
                    </span>
                  </CommandItem>
                </CommandGroup>
              )}
              {isPending && <p className="px-3 pb-3 text-xs text-muted-foreground">Loading namespaces…</p>}
              {isError && !offerTyped && (
                <p className="px-3 pb-3 text-xs text-muted-foreground">These credentials may not list namespaces. Type the one you can read.</p>
              )}
              {!isError && !isPending && !offerTyped && (
                <CommandEmpty className="px-3 py-4 text-center text-xs text-muted-foreground">
                  {typed && !isNamespace(typed) ? "Not a namespace name." : "No namespace matches."}
                </CommandEmpty>
              )}
            </CommandList>
          </Command>
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}
