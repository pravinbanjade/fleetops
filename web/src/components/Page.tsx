import { useEffect, type ReactNode } from "react";

/** Names the browser tab "<title> · FleetOps" while the page is shown. */
export function useDocumentTitle(title: string | undefined) {
  useEffect(() => {
    document.title = title ? `${title} · FleetOps` : "FleetOps";
  }, [title]);
}

/**
 * A page's title row, with its actions on the right and an optional line
 * of facts beneath the title.
 */
export function Page({
  title,
  kicker,
  meta,
  actions,
  children,
}: {
  title: ReactNode;
  /** A small grey line above the title naming what kind of page this is: "Docker host", "Deployment". */
  kicker?: ReactNode;
  /** The status line under the title: the count, and the one thing that needs attention. */
  meta?: ReactNode;
  actions?: ReactNode;
  children: ReactNode;
}) {
  useDocumentTitle(typeof title === "string" ? title : undefined);
  return (
    <div className="grid grid-cols-[minmax(0,1fr)] gap-5 px-4 pt-3 pb-10 md:px-6">
      <div className="flex flex-wrap items-end justify-between gap-x-4 gap-y-3">
        <div className="grid min-w-0 gap-1">
          {kicker && <div className="text-xs text-meta">{kicker}</div>}
          <h1 className="min-w-0 truncate text-[1.75rem] leading-tight font-bold tracking-[-0.02em]">{title}</h1>
          {meta && <div className="text-sm text-muted-foreground" data-numeric>{meta}</div>}
        </div>
        {actions && <div className="ml-auto flex flex-wrap items-center justify-end gap-2">{actions}</div>}
      </div>
      {children}
    </div>
  );
}
