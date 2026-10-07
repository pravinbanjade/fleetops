import type { KeyboardEvent } from "react";

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
