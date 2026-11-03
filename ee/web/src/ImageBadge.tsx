import { useQuery } from "@tanstack/react-query";
import { badgesQuery, SEVERITY_LABEL, STATE_LABEL } from "./security";

/** The image list's Security cell: the worst fixable severity, or why there is none. */
export function ImageBadge({ env, imageId }: { env: string; imageId: string }) {
  const b = useQuery(badgesQuery(env)).data?.find((x) => x.image_id === imageId);
  if (!b) return null;
  if (!b.has_result || b.state === "failed" || b.state === "scanner_unavailable") {
    return <span className="rounded-full bg-step px-2 py-0.5 text-[11px] text-ink-muted">{STATE_LABEL[b.state]}</span>;
  }
  if (!b.severity) return <span className="text-xs text-meta">No fixable issues</span>;
  return (
    <span className={`text-sm font-medium ${b.severity === "critical" ? "text-fall" : "text-state-wait"}`}>
      ● {SEVERITY_LABEL[b.severity]} {b.fixable}
    </span>
  );
}
