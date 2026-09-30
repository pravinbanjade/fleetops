import type { ReactNode } from "react";
import { ApiError } from "@/api/client";
import { errorMessage } from "@/lib/errors";

/** Label/value pairs in two columns. Empty values are left out. */
export function Details({ items, label }: { items: [string, ReactNode][]; label?: string }) {
  const shown = items.filter(([, v]) => v !== undefined && v !== null && v !== "");
  return (
    <dl aria-label={label} className="grid grid-cols-[max-content_1fr] gap-x-6 gap-y-2 text-sm">
      {shown.map(([k, v]) => (
        <div key={k} className="contents">
          <dt className="text-muted-foreground">{k}</dt>
          <dd className="min-w-0 break-words">{v}</dd>
        </div>
      ))}
    </dl>
  );
}

/** A detail page's failed query: "no longer exists" for a 404, the error otherwise. */
export function DetailError({ error, what, back }: { error: unknown; what: string; back: ReactNode }) {
  const gone = error instanceof ApiError && error.status === 404;
  return (
    <div className="grid gap-2 p-6">
      <p role="alert" className="text-sm text-destructive">
        {gone ? `This ${what} no longer exists.` : errorMessage(error)}
      </p>
      <div className="text-sm">{back}</div>
    </div>
  );
}
