import type { ReactNode } from "react";

/** A page's title row, with its actions on the right. */
export function Page({ title, actions, children }: { title: ReactNode; actions?: ReactNode; children: ReactNode }) {
  return (
    <div className="grid grid-cols-[minmax(0,1fr)] gap-4 p-6">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h1 className="min-w-0 truncate text-2xl font-semibold">{title}</h1>
        {actions && <div className="flex flex-wrap items-center gap-2">{actions}</div>}
      </div>
      {children}
    </div>
  );
}
