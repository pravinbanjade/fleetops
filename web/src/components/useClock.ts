import { useEffect, useState } from "react";

/** The time in unix seconds, refreshed every `every` milliseconds, for "in the last N minutes" counts. */
export function useClock(every = 10_000): number {
  const [now, setNow] = useState(() => Date.now() / 1000);
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now() / 1000), every);
    return () => clearInterval(t);
  }, [every]);
  return now;
}
