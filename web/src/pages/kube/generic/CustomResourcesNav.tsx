import { useQuery } from "@tanstack/react-query";
import { Link, useLocation } from "@tanstack/react-router";
import { ChevronRight, Puzzle } from "lucide-react";
import { useState, type ReactNode } from "react";
import type { ApiKind } from "@/api/gen/types.gen";
import { discoveryQuery } from "@/api/queries";
import { LINK, useGroupToggles } from "@/components/sidebar";
import { cn } from "@/lib/utils";
import { refOf, type Ref } from "./ref";

const DEFS: Ref = { group: "apiextensions.k8s.io", version: "v1", resource: "customresourcedefinitions" };

/** API groups with CRD-backed kinds, by name; every other kind under "Other APIs". */
function byGroup(kinds: ApiKind[]) {
  const crd = new Map<string, ApiKind[]>();
  const other: ApiKind[] = [];
  for (const k of kinds) {
    if (k.api.group === DEFS.group && k.api.resource === DEFS.resource) continue;
    if (k.crd) crd.set(k.api.group, [...(crd.get(k.api.group) ?? []), k]);
    else other.push(k);
  }
  return { crd: [...crd.entries()].sort(([a], [b]) => a.localeCompare(b)), other };
}

function Fold({ id, label, count, open, onToggle, nested, children }: {
  id: string;
  label: string;
  count?: number;
  open: boolean;
  onToggle: () => void;
  nested?: boolean;
  children: ReactNode;
}) {
  return (
    <div className="grid gap-0.5">
      <button
        type="button"
        aria-expanded={open}
        aria-controls={id}
        onClick={onToggle}
        className={cn(LINK, "w-full text-left outline-none focus-visible:ring-[3px] focus-visible:ring-ring/35", nested && "h-7 pl-2.5")}
      >
        {!nested && <Puzzle className="size-4 shrink-0" strokeWidth={1.75} />}
        <span className="truncate">{label}</span>
        {!open && count !== undefined && <span className="ml-auto font-mono text-[10px] text-muted-foreground/80 tabular-nums">{count}</span>}
        <ChevronRight
          aria-hidden
          className={cn("size-3.5 shrink-0 text-muted-foreground transition-transform duration-150 ease-out", open ? "ml-auto rotate-90" : "")}
          strokeWidth={1.75}
        />
      </button>
      {open && (
        <div id={id} className="mb-1 ml-[1.0625rem] grid gap-px border-l pl-1.5">
          {children}
        </div>
      )}
    </div>
  );
}

function KindLink({ env, to, label }: { env: string; to: Ref; label: string }) {
  return (
    <Link to="/envs/$env/k8s/r/$group/$version/$resource" params={{ env, ...to }} className={cn(LINK, "h-7 pl-2.5")}>
      {label}
    </Link>
  );
}

/**
 * The sidebar's Custom Resources group (spec 3g §2.5), built from discovery:
 * the CRD definitions, then each API group's kinds, folded per group.
 */
export function CustomResourcesNav({ env }: { env: string }) {
  const d = useQuery(discoveryQuery(env));
  const { pathname } = useLocation();
  const [toggles, setToggle] = useGroupToggles();
  const [dismissed, setDismissed] = useState(false);
  const here = pathname.includes("/k8s/r/");
  const open = toggles.crs ?? here;
  if (d.error)
    return (
      <Fold id="nav-crs" label="Custom Resources" open onToggle={() => setToggle("crs", !open)}>
        <p role="status" className="px-2.5 py-1 text-[11px] text-muted-foreground">
          Couldn't discover API resources.{" "}
          <button type="button" className="underline" onClick={() => void d.refetch()}>
            Retry
          </button>
        </p>
      </Fold>
    );
  if (!d.data) return null;
  const { crd, other } = byGroup(d.data.kinds);
  // Discovery leaves the CRDs out for grants that cannot open them.
  const defs = d.data.kinds.some((k) => k.api.group === DEFS.group && k.api.resource === DEFS.resource);
  const otherOpen = toggles["crs:other"] ?? false;
  return (
    <Fold id="nav-crs" label="Custom Resources" count={crd.length} open={open} onToggle={() => setToggle("crs", !open)}>
      {d.data.failed_groups.length > 0 && !dismissed && (
        <p role="status" className="flex items-start gap-1 px-2.5 py-1 text-[11px] text-muted-foreground">
          <span>Couldn't load: {d.data.failed_groups.join(", ")}</span>
          <button type="button" aria-label="Dismiss" className="ml-auto" onClick={() => setDismissed(true)}>
            ×
          </button>
        </p>
      )}
      {defs && <KindLink env={env} to={DEFS} label="Definitions" />}
      {crd.map(([group, kinds]) => {
        const gOpen = toggles[`crs:${group}`] ?? pathname.includes(`/k8s/r/${group}/`);
        return (
          <Fold key={group} id={`nav-crs-${group}`} label={group} count={kinds.length} open={gOpen} nested onToggle={() => setToggle(`crs:${group}`, !gOpen)}>
            {kinds.map((k) => (
              <KindLink key={k.api.resource} env={env} to={refOf(k)} label={k.kind} />
            ))}
          </Fold>
        );
      })}
      {other.length > 0 && (
        <Fold id="nav-crs-other" label="Other APIs" count={other.length} open={otherOpen} nested onToggle={() => setToggle("crs:other", !otherOpen)}>
          {other.map((k) => (
            <KindLink key={`${k.api.group}/${k.api.resource}`} env={env} to={refOf(k)} label={`${k.kind} (${k.api.group || "core"})`} />
          ))}
        </Fold>
      )}
    </Fold>
  );
}
