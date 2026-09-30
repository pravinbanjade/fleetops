import type { ReactNode } from "react";
import { ApiError } from "@/api/client";
import { errorMessage } from "@/lib/errors";

/** Label/value pairs in two columns. Empty values are left out. */
export function Details({ items, label }: { items: [string, ReactNode][]; label?: string }) {
  const shown = items.filter(([, v]) => v !== undefined && v !== null && v !== "");
  return (
    <dl
      aria-label={label}
      className="grid overflow-hidden rounded-lg border bg-card text-sm sm:grid-cols-[minmax(8rem,max-content)_minmax(0,1fr)]"
    >
      {shown.map(([k, v]) => (
        <div key={k} className="contents">
          <dt className="font-strip border-b px-4 pt-2 text-xs font-medium text-muted-foreground last-of-type:border-b-0 sm:border-r sm:bg-muted/40 sm:py-2.5 sm:text-sm sm:font-normal">
            {k}
          </dt>
          <dd className="min-w-0 border-b px-4 pb-2 break-words sm:py-2.5 [&:last-of-type]:border-b-0">{v}</dd>
        </div>
      ))}
    </dl>
  );
}

/** A detail page's failed query: "no longer exists" for a 404, the error otherwise. */
export function DetailError({ error, what, back }: { error: unknown; what: string; back: ReactNode }) {
  const gone = error instanceof ApiError && error.status === 404;
  return (
    <div className="m-4 grid max-w-xl gap-2 rounded-lg border border-destructive/40 bg-destructive/8 p-4 md:m-6">
      <p role="alert" className="text-sm font-medium text-destructive">
        {gone ? `This ${what} no longer exists.` : errorMessage(error)}
      </p>
      <div className="text-sm">{back}</div>
    </div>
  );
}
