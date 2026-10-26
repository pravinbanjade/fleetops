import { useEffect, type KeyboardEvent } from "react";
import { typing } from "./CommandPalette";

/** Moves focus between the links of a strip list with j/k and the arrow keys. */
export function walk(e: KeyboardEvent<HTMLElement>) {
  const next = e.key === "j" || e.key === "ArrowDown";
  const prev = e.key === "k" || e.key === "ArrowUp";
  if (!next && !prev) return;
  const links = [...e.currentTarget.querySelectorAll<HTMLAnchorElement>("a[data-strip]")];
  const at = links.indexOf(document.activeElement as HTMLAnchorElement);
  const to = links[Math.min(links.length - 1, Math.max(0, at + (next ? 1 : -1)))];
  if (to) {
    e.preventDefault();
    to.focus();
  }
}

/**
 * `j` or `k` from anywhere outside a strip list focuses the page's first
 * strip, so the lists' own walking takes over without a Tab hunt first.
 */
export function useWalkKeys() {
  useEffect(() => {
    const onKey = (e: globalThis.KeyboardEvent) => {
      // A list already walked this press, or the `g` chord used it.
      if (e.defaultPrevented || typing(e) || e.metaKey || e.ctrlKey || e.altKey) return;
      if (e.key !== "j" && e.key !== "k") return;
      const first = document.querySelector<HTMLAnchorElement>("main a[data-strip]");
      if (first) {
        e.preventDefault();
        first.focus();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
}
