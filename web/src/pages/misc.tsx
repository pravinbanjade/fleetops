import { Link } from "@tanstack/react-router";
import { MapPinOff, Plus, Unplug } from "lucide-react";
import type { LucideIcon } from "lucide-react";
import type { ReactNode } from "react";
import { useSession } from "@/auth/session";
import { buttonVariants } from "@/components/ui/button";

function Notice({ icon: Icon, title, children }: { icon: LucideIcon; title: string; children: ReactNode }) {
  return (
    <div className="grid min-h-[60svh] place-items-center p-6">
      <div className="grid max-w-md justify-items-center gap-3 text-center">
        <span className="flex size-11 items-center justify-center rounded-lg bg-raised text-muted-foreground">
          <Icon aria-hidden className="size-5" strokeWidth={1.75} />
        </span>
        <h1 className="tracking-tight text-xl font-semibold">{title}</h1>
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
  const { user } = useSession();
  if (user.role !== "admin") {
    return (
      <Notice icon={Unplug} title="No environments">
        <p className="text-sm text-muted-foreground">You don't have access to any environments yet. Ask an admin.</p>
      </Notice>
    );
  }
  return (
    <Notice icon={Unplug} title="Add your first environment">
      <p className="text-sm text-muted-foreground">
        Connect a Kubernetes cluster, a Docker host or a Docker Swarm. Agents connect remote ones without opening a port on them.
      </p>
      <Link to="/admin/envs" search={{ add: true }} className={buttonVariants({ className: "mt-2" })}>
        <Plus aria-hidden />
        Add environment
      </Link>
    </Notice>
  );
}
