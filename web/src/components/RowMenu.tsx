import { EllipsisVertical, type LucideIcon } from "lucide-react";
import { Fragment } from "react";
import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { cn } from "@/lib/utils";

/** One thing that can be done from a list: to a row from its menu, or to a selection from the bar. */
export interface QuickItem {
  id: string;
  label: string;
  icon: LucideIcon;
  onSelect: () => void;
  /** How many of the selection it applies to; shown when more than one. */
  count?: number;
  /** Destroys something: set apart, last, in fail ink. Its confirmation is the red one. */
  danger?: boolean;
  disabled?: boolean;
}

const DANGER = "text-state-fail-text hover:text-state-fail-text";

/** The selection bar's buttons: one per action, icon first, with the count it applies to. */
export function QuickButtons({ items }: { items: QuickItem[] }) {
  return items.map((it) => (
    <Button key={it.id} variant="outline" size="sm" disabled={it.disabled} className={cn(it.danger && DANGER)} onClick={it.onSelect}>
      <it.icon aria-hidden />
      {it.label}
      {it.count !== undefined && it.count > 1 ? ` ${it.count}` : ""}
    </Button>
  ));
}

/** A row's ⋮ menu: what can be done to this one object, named for it. */
export function RowMenu({ label, items }: { label: string; items: QuickItem[] }) {
  if (items.length === 0) return null;
  return (
    <DropdownMenu modal={false}>
      <DropdownMenuTrigger asChild>
        <Button variant="ghost" size="icon-sm" aria-label={`Actions for ${label}`} className="text-muted-foreground data-[state=open]:bg-accent data-[state=open]:text-foreground">
          <EllipsisVertical />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="min-w-44">
        {items.map((it, i) => (
          <Fragment key={it.id}>
            {it.danger && i > 0 && <DropdownMenuSeparator />}
            <DropdownMenuItem disabled={it.disabled} onSelect={it.onSelect} className={cn(it.danger && "text-state-fail-text focus:text-state-fail-text [&_svg]:text-state-fail-text")}>
              <it.icon aria-hidden />
              {it.label}
            </DropdownMenuItem>
          </Fragment>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
