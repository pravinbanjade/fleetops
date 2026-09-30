import { Link } from "@tanstack/react-router";
import { MapPinOff, Unplug } from "lucide-react";
import type { LucideIcon } from "lucide-react";
import type { ReactNode } from "react";

function Notice({ icon: Icon, title, children }: { icon: LucideIcon; title: string; children: ReactNode }) {
  return (
    <div className="grid min-h-[60svh] place-items-center p-6">
      <div className="grid max-w-md justify-items-center gap-3 text-center">
        <span className="flex size-11 items-center justify-center rounded-lg border bg-card text-muted-foreground">
          <Icon aria-hidden className="size-5" strokeWidth={1.75} />
        </span>
        <h1 className="font-callsign text-xl font-semibold">{title}</h1>
        {children}
      </div>
    </div>
  );
}

export function NotFoundPage() {
  return (
    <Notice icon={MapPinOff} title="Page not found">
      <Link to="/" className="text-sm underline underline-offset-4">
        Go to the start page
      </Link>
    </Notice>
  );
}

export function NoEnvironmentsPage() {
  return (
    <Notice icon={Unplug} title="No environments">
      <p className="text-sm text-muted-foreground">
        FleetOps registers the local Docker socket when it can reach one at start. Check that the socket is mounted and
        readable, then restart FleetOps.
      </p>
    </Notice>
  );
}
