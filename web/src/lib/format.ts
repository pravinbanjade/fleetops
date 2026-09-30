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
