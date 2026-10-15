import type { LucideIcon } from "lucide-react";
import { useId, type ReactNode } from "react";

/**
 * A panel of the board: a titled strip header (16px muted icon, title,
 * muted aside, then actions on the right) over its content.
 */
export function Panel({
  title,
  icon: Icon,
  aside,
  actions,
  flush,
  children,
}: {
  title: ReactNode;
  icon?: LucideIcon;
  aside?: ReactNode;
  actions?: ReactNode;
  flush?: boolean;
  children: ReactNode;
}) {
  const id = useId();
  return (
    <section aria-labelledby={id} className="grid content-start overflow-clip rounded-lg border bg-card">
      <div className="flex min-h-11 flex-wrap items-center gap-x-4 gap-y-1 border-b px-4 py-1.5">
        <h2 id={id} className="flex min-w-0 items-center gap-2 text-sm font-semibold">
          {Icon && <Icon aria-hidden className="size-4 shrink-0 text-muted-foreground" strokeWidth={1.75} />}
          {title}
        </h2>
        {aside && <span className="min-w-0 text-xs text-muted-foreground">{aside}</span>}
        {actions && <div className="ml-auto flex items-center gap-2">{actions}</div>}
      </div>
      <div className={flush ? undefined : "p-4"}>{children}</div>
    </section>
  );
}
