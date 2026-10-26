import { Link } from "@tanstack/react-router";
import { ArrowUpRight } from "lucide-react";
import type { ReactNode } from "react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { NowPanel } from "./now";

export type NowFact = { label: string; value: ReactNode };
export type NowRow = { key: string; label: ReactNode; aside?: ReactNode; failing?: boolean; link?: { to: string; params: Record<string, string> } };

/** A small raised tile: a label over a figure. */
export function Fact({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="grid min-w-0 gap-0.5 rounded-md bg-[#1c1c1c] px-2.5 py-2">
      <span className="truncate text-xs text-meta">{label}</span>
      <span className="truncate text-base font-semibold">{children}</span>
    </div>
  );
}

/**
 * An object in the "now playing" panel, in the container panel's shape:
 * who it is, two or three facts, the live list that matters for it, and
 * Open first among its actions.
 */
export function ObjectNow({
  title,
  subtitle,
  facts,
  list,
  open,
  actions,
  children,
}: {
  title: ReactNode;
  subtitle?: ReactNode;
  facts: NowFact[];
  list?: { title: string; rows: NowRow[]; empty: string };
  open: { to: string; params: Record<string, string> };
  actions?: ReactNode;
  children?: ReactNode;
}) {
  return (
    <NowPanel title={title} subtitle={subtitle}>
      <div className={cn("grid gap-2", facts.length >= 3 ? "grid-cols-3" : "grid-cols-2")}>
        {facts.map((f) => (
          <Fact key={f.label} label={f.label}>
            {f.value}
          </Fact>
        ))}
      </div>
      {list && (
        <section aria-label={list.title} className="grid min-h-0 content-start gap-1 overflow-y-auto">
          <h3 className="text-xs text-meta">
            {list.title} <span className="font-figure">{list.rows.length}</span>
          </h3>
          {list.rows.length === 0 ? (
            <p className="text-sm text-meta">{list.empty}</p>
          ) : (
            <ul className="grid">
              {list.rows.map((r) => (
                <li key={r.key} className="flex h-9 min-w-0 items-center gap-3 rounded-md px-2 text-sm hover:bg-accent">
                  {r.link ? (
                    <Link to={r.link.to as never} params={r.link.params as never} className={cn("min-w-0 flex-1 truncate font-ident hover:underline", r.failing && "text-fall")}>
                      {r.label}
                    </Link>
                  ) : (
                    <span className={cn("min-w-0 flex-1 truncate font-ident", r.failing && "text-fall")}>{r.label}</span>
                  )}
                  {r.aside && <span className="shrink-0 text-xs text-muted-foreground">{r.aside}</span>}
                </li>
              ))}
            </ul>
          )}
        </section>
      )}
      {children}
      <div className="mt-auto flex flex-wrap gap-2">
        <Button asChild size="sm">
          <Link to={open.to as never} params={open.params as never}>
            <ArrowUpRight />
            Open
          </Link>
        </Button>
        {actions}
      </div>
    </NowPanel>
  );
}
