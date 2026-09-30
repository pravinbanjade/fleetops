const UNITS = ["B", "KB", "MB", "GB", "TB", "PB"];

/** Bytes in powers of 1000 with one decimal place above 1 KB: 1.5 GB. */
export function formatBytes(n: number | null | undefined): string {
  if (n == null || !Number.isFinite(n) || n < 0) return "—";
  let i = 0;
  while (n >= 1000 && i < UNITS.length - 1) {
    n /= 1000;
    i++;
  }
  return i === 0 ? `${n} B` : `${n.toFixed(1)} ${UNITS[i]}`;
}

/** Unix seconds as a local date and time. */
export function formatTime(unix: number): string {
  return new Date(unix * 1000).toLocaleString();
}

/** How long ago a Unix-seconds time was: "just now", "5 minutes ago", "3 days ago"; older than a month, the date. */
export function formatAgo(unix: number, now = Date.now()): string {
  const s = Math.max(0, Math.round(now / 1000 - unix));
  const ago = (n: number, unit: string) => `${n} ${unit}${n === 1 ? "" : "s"} ago`;
  if (s < 60) return "just now";
  if (s < 3600) return ago(Math.floor(s / 60), "minute");
  if (s < 86400) return ago(Math.floor(s / 3600), "hour");
  if (s < 30 * 86400) return ago(Math.floor(s / 86400), "day");
  return new Date(unix * 1000).toLocaleDateString();
}

/** A Docker id as the CLI shows it: 12 hex characters, without the `sha256:` prefix. */
export function shortId(id: string): string {
  return id.replace(/^sha256:/, "").slice(0, 12);
}

/** A CPU percentage as `docker stats` shows it, which can pass 100 on several cores: 0.4%, 12%, 250%. */
export function formatPercent(n: number): string {
  if (!Number.isFinite(n) || n < 0) return "—";
  return n < 10 ? `${n.toFixed(1)}%` : `${Math.round(n)}%`;
}
