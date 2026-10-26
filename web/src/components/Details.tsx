import { CircleAlert } from "lucide-react";
import type { ReactNode } from "react";
import { ApiError } from "@/api/client";
import { errorMessage } from "@/lib/errors";
import { useDocumentTitle } from "./Page";

/** Label/value pairs in two columns. Empty values are left out. */
export function Details({ items, label }: { items: [string, ReactNode][]; label?: string }) {
  const shown = items.filter(([, v]) => v !== undefined && v !== null && v !== "");
  return (
    <dl
      aria-label={label}
      className="grid overflow-hidden rounded-lg bg-[#1c1c1c] text-sm sm:grid-cols-[minmax(8rem,max-content)_minmax(0,1fr)]"
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

/**
 * A detail page's failed query: "no longer exists" for a 404, the error
 * otherwise. The headline is the page's h1 and its tab title, so the page
 * is announced and findable like any other.
 */
export function DetailError({ error, what, back }: { error: unknown; what: string; back: ReactNode }) {
  const gone = error instanceof ApiError && error.status === 404;
  const What = what.charAt(0).toUpperCase() + what.slice(1);
  useDocumentTitle(gone ? `${What} not found` : `${What} unavailable`);
  return (
    <div role="alert" className="m-4 grid max-w-xl gap-1.5 rounded-lg bg-destructive/8 p-4 md:m-6">
      {/* Ink text on the tint: destructive text on it falls just under 4.5:1 in light. */}
      <h1 className="flex items-start gap-2 text-sm font-semibold">
        <CircleAlert aria-hidden className="mt-px size-4 shrink-0 text-destructive" />
        {gone ? `This ${what} no longer exists.` : `This ${what} could not be loaded.`}
      </h1>
      {!gone && <p className="pl-6 font-mono text-xs break-words text-muted-foreground">{errorMessage(error)}</p>}
      <div className="pt-0.5 pl-6 text-sm">{back}</div>
    </div>
  );
}
