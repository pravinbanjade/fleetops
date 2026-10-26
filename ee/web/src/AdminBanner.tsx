import { Link } from "@tanstack/react-router";
import { TriangleAlert, X } from "lucide-react";
import { useState } from "react";
import { Button } from "@/components/ui/button";

const KEY = "fleetops-dismissed-banners";

function dismissed(): string[] {
  try {
    return JSON.parse(sessionStorage.getItem(KEY) ?? "[]") as string[];
  } catch {
    return [];
  }
}

/**
 * An admin's banner across the top of every page. Dismissing hides it for
 * this tab's session, and only this text: a new problem shows again.
 */
export function AdminBanner({ text, to, link }: { text: string; to: string; link: string }) {
  const [hidden, setHidden] = useState(() => dismissed().includes(text));
  if (hidden) return null;
  const dismiss = () => {
    try {
      sessionStorage.setItem(KEY, JSON.stringify([...dismissed(), text]));
    } catch {
      // Without storage it stays hidden until the page reloads.
    }
    setHidden(true);
  };
  return (
    <div role="status" className="flex items-center gap-3 border-b border-state-wait/45 bg-state-wait/10 px-4 py-1.5 text-sm">
      <TriangleAlert aria-hidden className="size-4 shrink-0 text-state-wait-text" strokeWidth={1.75} />
      <p className="min-w-0 flex-1">
        {text}{" "}
        <Link to={to as never} className="font-medium underline underline-offset-2">
          {link}
        </Link>
      </p>
      <Button variant="ghost" size="icon-sm" aria-label="Dismiss for this session" onClick={dismiss}>
        <X />
      </Button>
    </div>
  );
}
