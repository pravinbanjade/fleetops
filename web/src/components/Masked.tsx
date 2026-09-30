import { useState } from "react";
import { MASK } from "@/lib/secrets";

/** A secret value shown as a mask until clicked. */
export function Masked({ name, value }: { name: string; value: string }) {
  const [shown, setShown] = useState(false);
  if (shown) return <>{value}</>;
  return (
    <button
      type="button"
      className="text-muted-foreground hover:underline"
      aria-label={`Show ${name}`}
      onClick={() => setShown(true)}
    >
      {MASK}
    </button>
  );
}
