import { useEffect, useState } from "react";

/**
 * Whether an element is on screen (or within 200px of it). Where the
 * browser has no IntersectionObserver, everything counts as in view.
 */
export function useInView<T extends Element>(): [(el: T | null) => void, boolean] {
  const [el, setEl] = useState<T | null>(null);
  const [inView, setInView] = useState(() => typeof IntersectionObserver === "undefined");
  useEffect(() => {
    if (!el || typeof IntersectionObserver === "undefined") return;
    const io = new IntersectionObserver(([e]) => setInView(e.isIntersecting), { rootMargin: "200px" });
    io.observe(el);
    return () => io.disconnect();
  }, [el]);
  return [setEl, inView];
}
