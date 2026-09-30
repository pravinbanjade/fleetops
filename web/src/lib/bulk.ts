import { errorMessage } from "./errors";

export interface BulkResult {
  ok: number;
  failed: { name: string; error: unknown }[];
}

/** Runs `fn` on every item at once and collects which ones failed. */
export async function runBulk<T>(items: T[], name: (item: T) => string, fn: (item: T) => Promise<unknown>) {
  const results = await Promise.allSettled(items.map(fn));
  const out: BulkResult = { ok: 0, failed: [] };
  results.forEach((r, i) => {
    if (r.status === "fulfilled") out.ok++;
    else out.failed.push({ name: name(items[i]), error: r.reason });
  });
  return out;
}

/** "Stopped 3 containers", or "Stopped 2 of 3 containers. web: is not running". */
export function bulkMessage(done: string, noun: string, r: BulkResult) {
  const total = r.ok + r.failed.length;
  const what = `${total} ${noun}${total === 1 ? "" : "s"}`;
  if (r.failed.length === 0) return `${done} ${what}`;
  const errors = r.failed.map((f) => `${f.name}: ${errorMessage(f.error)}`).join("; ");
  return `${done} ${r.ok} of ${what}. ${errors}`;
}
