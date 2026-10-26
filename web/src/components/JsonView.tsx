import { Copy, Eye } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";

/**
 * Raw JSON, pretty-printed, with a copy button. With `masked` (the same
 * value with its secrets replaced), that is shown and copied until the
 * viewer asks for the secrets.
 */
export function JsonView({ value, masked, label }: { value: unknown; masked?: unknown; label: string }) {
  const [reveal, setReveal] = useState(false);
  const text = JSON.stringify(masked !== undefined && !reveal ? masked : value, null, 2);
  const copy = async () => {
    await navigator.clipboard.writeText(text);
    toast.success("Copied");
  };
  return (
    <div className="relative">
      <div className="absolute top-2 right-2 flex gap-2">
        {masked !== undefined && !reveal && (
          <Button variant="outline" size="sm" onClick={() => setReveal(true)}>
            <Eye />
            Show secrets
          </Button>
        )}
        <Button variant="outline" size="sm" onClick={() => void copy()}>
          <Copy />
          Copy
        </Button>
      </div>
      <pre aria-label={label} className="max-h-[70vh] overflow-auto rounded-md bg-[#0a0a0a] p-4 font-mono text-xs">
        {text}
      </pre>
    </div>
  );
}
