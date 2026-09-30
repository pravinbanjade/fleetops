import { Copy } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";

/** Raw JSON, pretty-printed, with a copy button. */
export function JsonView({ value, label }: { value: unknown; label: string }) {
  const text = JSON.stringify(value, null, 2);
  const copy = async () => {
    await navigator.clipboard.writeText(text);
    toast.success("Copied");
  };
  return (
    <div className="relative">
      <Button variant="outline" size="sm" className="absolute top-2 right-2" onClick={() => void copy()}>
        <Copy />
        Copy
      </Button>
      <pre aria-label={label} className="max-h-[70vh] overflow-auto rounded-md border bg-muted/40 p-4 font-mono text-xs">
        {text}
      </pre>
    </div>
  );
}
