import { ArrowLeft, X } from "lucide-react";
import { Dialog as DialogPrimitive } from "radix-ui";
import { createContext, useCallback, useContext, useEffect, useMemo, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { Button } from "@/components/ui/button";
import { typing } from "../CommandPalette";
import { useMedia } from "../useMedia";

/** Below this width the panel is a sheet over the list instead of a column beside it. */
export const SHEET_QUERY = "(max-width: 1023px)";

interface Ctl {
  id: string | null;
  open: (id: string) => void;
  close: () => void;
  slot: HTMLElement | null;
  setSlot: (el: HTMLElement | null) => void;
  shown: boolean;
  mount: () => () => void;
}

const Ctx = createContext<Ctl | null>(null);

/**
 * Holds what the "now playing" panel shows: the selected object's id, for
 * the page it was chosen on. Moving to another page clears it, and so does
 * Escape (unless a dialog or menu has the keyboard).
 */
export function NowHost({ path, children }: { path: string; children: ReactNode }) {
  const [sel, setSel] = useState<{ path: string; id: string } | null>(null);
  const id = sel?.path === path ? sel.id : null;
  const [slot, setSlot] = useState<HTMLElement | null>(null);
  const [mounts, setMounts] = useState(0);
  const open = useCallback((next: string) => setSel({ path, id: next }), [path]);
  const close = useCallback(() => setSel(null), []);
  const mount = useCallback(() => {
    setMounts((n) => n + 1);
    return () => setMounts((n) => n - 1);
  }, []);
  useEffect(() => {
    if (!id) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || e.defaultPrevented || typing(e)) return;
      if (document.querySelector('[role="dialog"]:not([data-now]), [role="menu"]')) return;
      close();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [id, close]);
  const value = useMemo(() => ({ id, open, close, slot, setSlot, shown: mounts > 0, mount }), [id, open, close, slot, mounts, mount]);
  return <Ctx value={value}>{children}</Ctx>;
}

function useCtl(): Ctl {
  const ctl = useContext(Ctx);
  if (!ctl) throw new Error("useNow outside NowHost");
  return ctl;
}

/** The selected object of this page, and how to change it. */
export function useNow(): { id: string | null; open: (id: string) => void; close: () => void } {
  const { id, open, close } = useCtl();
  return { id, open, close };
}

/** For the shell: where the panel renders, and whether one is showing. */
export function useNowFrame(): { setSlot: (el: HTMLElement | null) => void; shown: boolean } {
  const { setSlot, shown } = useCtl();
  return { setSlot, shown };
}

/**
 * The row the panel shows. When the list has loaded and the row is gone
 * (removed, deleted), the panel closes rather than show a dead object.
 */
export function useNowRow<T>(rows: T[] | undefined, getId: (row: T) => string): T | undefined {
  const { id, close } = useCtl();
  const row = id && rows ? rows.find((r) => getId(r) === id) : undefined;
  const gone = !!id && !!rows && !row;
  useEffect(() => {
    if (gone) close();
  }, [gone, close]);
  return row;
}

/**
 * The "now playing" panel: the selected object, live, beside the list. On a
 * narrow window it is a sheet over the list instead.
 */
export function NowPanel({ title, subtitle, children }: { title: ReactNode; subtitle?: ReactNode; children: ReactNode }) {
  const { slot, close, mount } = useCtl();
  const sheet = useMedia(SHEET_QUERY);
  const phone = useMedia("(max-width: 767px)");
  useEffect(() => mount(), [mount]);
  const body = (
    <div className="flex h-full min-h-0 flex-col gap-4 p-4 motion-safe:animate-in motion-safe:fade-in-0 motion-safe:slide-in-from-right-6 motion-safe:duration-250 motion-safe:ease-drawer">
      <header className="flex items-start gap-2">
        <div className="grid min-w-0 flex-1 gap-0.5">
          <h2 className="truncate text-[17px] leading-tight font-semibold">{title}</h2>
          {subtitle && <div className="truncate text-sm text-muted-foreground">{subtitle}</div>}
        </div>
        {phone ? (
          // Full screen on a phone: back to the list, which kept its scroll underneath.
          <Button variant="secondary" size="sm" className="order-first" onClick={close}>
            <ArrowLeft aria-hidden />
            Back
          </Button>
        ) : (
          <Button variant="ghost" size="icon-sm" aria-label="Close panel" title="Close (Esc)" onClick={close}>
            <X />
          </Button>
        )}
      </header>
      {children}
    </div>
  );
  if (sheet) {
    return (
      <DialogPrimitive.Root open onOpenChange={(o) => !o && close()}>
        <DialogPrimitive.Portal>
          <DialogPrimitive.Overlay className="fixed inset-0 z-50 bg-black/70 data-[state=open]:animate-in data-[state=open]:fade-in-0" />
          <DialogPrimitive.Content
            data-now
            aria-describedby={undefined}
            className="fixed inset-y-0 right-0 z-50 w-[min(28rem,100vw)] bg-card shadow-[0_0_64px_rgb(0_0_0/0.8)] outline-none max-md:w-full"
          >
            <DialogPrimitive.Title className="sr-only">{typeof title === "string" ? title : "Selected"}</DialogPrimitive.Title>
            {body}
          </DialogPrimitive.Content>
        </DialogPrimitive.Portal>
      </DialogPrimitive.Root>
    );
  }
  return slot ? createPortal(body, slot) : null;
}
