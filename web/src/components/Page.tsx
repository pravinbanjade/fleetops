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
  meta,
  actions,
  children,
}: {
  title: ReactNode;
  meta?: ReactNode;
  actions?: ReactNode;
  children: ReactNode;
}) {
  useDocumentTitle(typeof title === "string" ? title : undefined);
  return (
    <div className="grid grid-cols-[minmax(0,1fr)] gap-5 px-4 pt-5 pb-10 md:px-6">
      <div className="flex flex-wrap items-end justify-between gap-x-4 gap-y-3">
        <div className="grid min-w-0 gap-1">
          <h1 className="font-callsign min-w-0 truncate text-[1.375rem] leading-tight font-semibold">{title}</h1>
          {meta && <div className="text-sm text-muted-foreground">{meta}</div>}
        </div>
        {actions && <div className="flex flex-wrap items-center gap-2">{actions}</div>}
      </div>
      {children}
    </div>
  );
}
