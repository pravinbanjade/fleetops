import { StateBadge, type Tone } from "@/components/StateBadge";

function tone(status: string): Tone {
  if (status === "deployed") return "run";
  if (status.startsWith("pending-") || status === "uninstalling") return "wait";
  if (status === "failed" || status === "unreadable") return "fail";
  return "idle";
}

/** A release's status in Helm's own words; an unreadable one says why on hover. */
export function HelmStatus({ status, error }: { status: string; error?: string | null }) {
  return (
    <span title={error ?? undefined}>
      <StateBadge state={status} tone={tone(status)} />
    </span>
  );
}
