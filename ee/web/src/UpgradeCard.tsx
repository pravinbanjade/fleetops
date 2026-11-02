import { useQuery } from "@tanstack/react-query";
import { Sparkles } from "lucide-react";
import { editionQuery, meQuery } from "@/api/queries";
import { buttonVariants } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { FEATURE_LABELS, PRICING_URL } from "./plans";

const list = new Intl.ListFormat("en", { type: "conjunction" }).format(Object.values(FEATURE_LABELS));
const SUMMARY = `${list[0].toUpperCase()}${list.slice(1)}.`;

/** For admins on Community: what Enterprise adds, and the way to the pricing page. */
export function UpgradeCard({ collapsed }: { collapsed: boolean }) {
  const { data: me } = useQuery(meQuery);
  const { data: edition } = useQuery(editionQuery);
  if (me?.user.role !== "admin" || edition?.edition !== "community") return null;
  if (collapsed) {
    return (
      <a
        href={PRICING_URL}
        target="_blank"
        rel="noopener noreferrer"
        aria-label="Upgrade to Enterprise"
        title="Upgrade to Enterprise"
        className={cn(buttonVariants({ variant: "ghost", size: "icon" }), "justify-self-center")}
      >
        <Sparkles aria-hidden className="size-4" strokeWidth={1.75} />
      </a>
    );
  }
  return (
    <div className="grid gap-2 rounded-md bg-raised p-3">
      <div className="text-sm font-semibold">Enterprise</div>
      <p className="text-xs text-muted-foreground">{SUMMARY}</p>
      <a href={PRICING_URL} target="_blank" rel="noopener noreferrer" className={cn(buttonVariants({ size: "sm" }), "w-full")}>
        Upgrade
      </a>
    </div>
  );
}
