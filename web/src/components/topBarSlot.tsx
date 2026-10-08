import { createContext, useContext, type ReactNode } from "react";
import { createPortal } from "react-dom";

/** Where a page may put controls of its own in the top bar. */
export const TopBarSlot = createContext<HTMLElement | null>(null);

/** Renders `children` in the top bar, beside the live state. */
export function TopBarActions({ children }: { children: ReactNode }) {
  const slot = useContext(TopBarSlot);
  return slot ? createPortal(children, slot) : null;
}
