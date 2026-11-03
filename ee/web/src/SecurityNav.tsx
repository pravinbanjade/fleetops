import { Link } from "@tanstack/react-router";
import { ShieldAlert } from "lucide-react";
import { LINK } from "@/components/sidebar";
import { cn } from "@/lib/utils";

/** Sidebar: the fleet-wide patch queue. */
export function SecurityNav({ collapsed }: { collapsed: boolean }) {
  return (
    <nav className="grid w-full gap-0.5" aria-label="Fleet">
      <Link
        to={"/security" as never}
        aria-label={collapsed ? "Security" : undefined}
        title={collapsed ? "Security" : undefined}
        className={cn(LINK, collapsed && "justify-center px-0")}
      >
        <ShieldAlert className="size-4 shrink-0" strokeWidth={1.75} />
        {!collapsed && "Security"}
      </Link>
    </nav>
  );
}
