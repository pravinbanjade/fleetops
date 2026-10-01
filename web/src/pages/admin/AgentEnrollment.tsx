import { useEffect, useState } from "react";
import { toast } from "sonner";
import type { Enrollment } from "@/api/gen/types.gen";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";

const minutesLeft = (expiresAt: number) => Math.max(0, Math.ceil((expiresAt - Date.now() / 1000) / 60));

/** The one-time join command for an agent. It is not kept anywhere once closed. */
export function AgentEnrollment({ enrollment, onClose }: { enrollment: Enrollment; onClose: () => void }) {
  const [left, setLeft] = useState(() => minutesLeft(enrollment.expires_at));
  useEffect(() => {
    const t = setInterval(() => setLeft(minutesLeft(enrollment.expires_at)), 30_000);
    return () => clearInterval(t);
  }, [enrollment.expires_at]);
  const copy = async () => {
    await navigator.clipboard.writeText(enrollment.command);
    toast.success("Copied");
  };
  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Run this on the remote host</DialogTitle>
          <DialogDescription>
            {left > 0 ? `Expires in ${left} min, works once.` : "This command has expired; rotate the key for a new one."}
          </DialogDescription>
        </DialogHeader>
        <pre className="max-w-full overflow-x-auto rounded-md bg-muted p-3 font-mono text-xs whitespace-pre">{enrollment.command}</pre>
        <p className="text-sm text-muted-foreground">
          The agent shows as online here within a few seconds of starting. Its key is stored in the fleetops-agent volume.
        </p>
        <DialogFooter>
          <Button variant="outline" onClick={() => void copy()}>
            Copy command
          </Button>
          <Button onClick={onClose}>Done</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
