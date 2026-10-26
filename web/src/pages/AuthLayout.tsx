import type { ReactNode } from "react";

export function AuthLayout({ title, description, children }: { title: string; description?: string; children: ReactNode }) {
  return (
    <div className="grid min-h-svh place-items-center bg-rail p-4 sm:p-6">
      <div className="grid w-full max-w-sm gap-6">
        <div className="flex items-center gap-2.5">
          <svg viewBox="0 0 32 32" className="size-8" aria-hidden>
            <rect width="32" height="32" rx="7" className="fill-foreground" />
            <path d="M8 10h16v3H11.5v3H21v3h-9.5v5H8z" className="fill-background" />
            <rect x="21" y="21" width="4" height="4" rx="1" className="fill-primary" />
          </svg>
          <span className="tracking-tight text-lg font-semibold">FleetOps</span>
        </div>
        <div className="grid gap-5 rounded-xl border bg-card p-6 shadow-[0_12px_40px_-16px_oklch(0_0_0/0.35)]">
          <div className="grid gap-1.5">
            <h1 className="tracking-tight text-xl leading-tight font-semibold">{title}</h1>
            {description && <p className="text-sm text-muted-foreground">{description}</p>}
          </div>
          {children}
        </div>
        <p className="text-xs text-muted-foreground">Open-source container management. Every action is audited.</p>
      </div>
    </div>
  );
}

export function FormError({ message }: { message: string | null }) {
  if (!message) return null;
  return (
    <p role="alert" className="rounded-md border border-destructive/40 bg-destructive/8 px-3 py-2 text-sm text-destructive">
      {message}
    </p>
  );
}
