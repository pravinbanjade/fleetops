import { Blobatar } from "@blobatar/react";
import { Bot } from "lucide-react";
import { cn } from "@/lib/utils";

const SIZE = { sm: 20, md: 28, lg: 40 } as const;
const TILE = { sm: "size-5 rounded-xs", md: "size-7 rounded-sm", lg: "size-10 rounded-md" } as const;

/**
 * A user's face: a blobatar drawn from their username, so the same person is
 * the same creature in the menu, the users list and the audit log. It sits
 * on a squared tile, like every identity mark on the board. The name always
 * appears beside it, so the picture is decorative.
 */
export function UserAvatar({ name, size = "md", className }: { name: string; size?: keyof typeof SIZE; className?: string }) {
  return (
    <span aria-hidden className={cn("inline-flex shrink-0 items-center justify-center border bg-muted/60", TILE[size], className)}>
      <Blobatar name={name} size={SIZE[size] - 2} alt="" draggable={false} />
    </span>
  );
}

/** The audit log's actor when no user did it: FleetOps itself. */
export function SystemAvatar({ size = "sm" }: { size?: keyof typeof SIZE }) {
  return (
    <span aria-hidden className={cn("inline-flex shrink-0 items-center justify-center border bg-muted/60 text-muted-foreground", TILE[size])}>
      <Bot className="size-3.5" />
    </span>
  );
}
