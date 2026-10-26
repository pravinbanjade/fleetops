import { useSyncExternalStore } from "react";

/** Whether a media query matches, kept current as the window changes. */
export function useMedia(query: string): boolean {
  return useSyncExternalStore(
    (on) => {
      const mq = window.matchMedia?.(query);
      mq?.addEventListener("change", on);
      return () => mq?.removeEventListener("change", on);
    },
    () => window.matchMedia?.(query).matches ?? false,
    () => false,
  );
}
