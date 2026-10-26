import { Link } from "@tanstack/react-router";
import { Lock } from "lucide-react";
import type { ReactNode } from "react";

/**
 * What a page can still do without a license: one strip, the same on every
 * Enterprise page, in the waiting hue rather than the alarm one.
 */
export function FrozenNotice({ children }: { children: ReactNode }) {
  return (
    <div
      role="status"
      className="flex flex-wrap items-center gap-x-3 gap-y-1 rounded-md border border-state-wait/45 bg-state-wait/10 px-3 py-2 text-sm"
    >
      <Lock aria-hidden className="size-4 shrink-0 text-state-wait-text" strokeWidth={1.75} />
      <span className="min-w-0 flex-1">{children}</span>
      <Link to={"/admin/license" as never} className="font-medium underline underline-offset-2">
        Manage license
      </Link>
    </div>
  );
}
