import { Mark } from "@/components/shell/Library";
import type { ReactNode } from "react";

export function AuthLayout({ title, description, children }: { title: string; description?: string; children: ReactNode }) {
  return (
    <div className="grid min-h-svh place-items-center bg-background p-4 sm:p-6">
      <div className="grid w-full max-w-sm gap-6">
        <div className="flex items-center gap-2.5">
          <Mark size="size-8" />
          <span className="text-lg font-semibold tracking-tight">FleetOps</span>
        </div>
        <div className="grid gap-5 rounded-[10px] bg-card p-6">
          <div className="grid gap-1.5">
            <h1 className="text-[1.75rem] leading-tight font-bold tracking-[-0.02em]">{title}</h1>
            {description && <p className="text-sm text-muted-foreground">{description}</p>}
          </div>
          {children}
        </div>
        <p className="text-xs text-meta">Open-source container management. Every action is audited.</p>
      </div>
    </div>
  );
}

export function FormError({ message }: { message: string | null }) {
  if (!message) return null;
  return (
    <p role="alert" className="rounded-md bg-destructive/8 px-3 py-2 text-sm text-destructive">
      {message}
    </p>
  );
}
