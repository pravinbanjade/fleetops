import { useState } from "react";

/** A sidebar row: icon, label, and the active page's cord highlight. */
export const LINK =
  "group/link relative flex h-8 items-center gap-2.5 rounded-md px-2 text-sm text-muted-foreground transition-colors hover:bg-accent hover:text-foreground data-[status=active]:bg-cord-soft data-[status=active]:font-medium data-[status=active]:text-foreground [&[data-status=active]_svg]:text-primary";

const GROUPS_KEY = "fleetops.nav.groups";

/** Which groups the operator opened or closed by hand; the rest follow the current page. */
export function useGroupToggles(): [Record<string, boolean>, (id: string, open: boolean) => void] {
  const [toggles, setToggles] = useState<Record<string, boolean>>(() => {
    try {
      const v: unknown = JSON.parse(localStorage.getItem(GROUPS_KEY) ?? "{}");
      return v && typeof v === "object" ? (v as Record<string, boolean>) : {};
    } catch {
      return {};
    }
  });
  const set = (id: string, open: boolean) =>
    setToggles((t) => {
      const next = { ...t, [id]: open };
      try {
        localStorage.setItem(GROUPS_KEY, JSON.stringify(next));
      } catch {
        /* storage blocked: the choice lasts this page only */
      }
      return next;
    });
  return [toggles, set];
}
