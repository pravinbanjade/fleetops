import { useState } from "react";
import { toast } from "sonner";
import { finishSwarmRotation } from "@/api/gen/sdk.gen";
import type { SwarmEnrollment as Enrollment } from "@/api/gen/types.gen";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { errorMessage } from "@/lib/errors";

const copy = async (text: string) => {
  await navigator.clipboard.writeText(text);
  toast.success("Copied");
};

function download(yaml: string) {
  const url = URL.createObjectURL(new Blob([yaml], { type: "application/yaml" }));
  const a = Object.assign(document.createElement("a"), { href: url, download: "fleetops-agent.yml" });
  a.click();
  URL.revokeObjectURL(url);
}

const pre = "max-h-56 max-w-full overflow-auto rounded-md bg-muted p-3 font-mono text-xs whitespace-pre";

/**
 * A Swarm environment's key, shown once, and the agent stack that uses it.
 * After a rotation it also offers to finish the rotation.
 */
export function SwarmEnrollment({
  env,
  enrollment,
  rotated = false,
  onClose,
}: {
  env: string;
  enrollment: Enrollment;
  rotated?: boolean;
  onClose: () => void;
}) {
  const [inside, setInside] = useState(false);
  const stack = (inside && enrollment.inside) || enrollment.outside;
  const finish = async () => {
    try {
      await finishSwarmRotation({ path: { env }, throwOnError: true });
      toast.success("Rotation finished: the old key no longer works");
    } catch (e) {
      toast.error(errorMessage(e));
    }
  };
  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>Start the agents on {env}</DialogTitle>
          <DialogDescription>The key is shown once. FleetOps keeps only a hash of it.</DialogDescription>
        </DialogHeader>
        <section className="grid gap-2">
          <h3 className="text-sm font-medium">1. Store the key as a Docker secret, on a manager</h3>
          <div className="flex items-center gap-2">
            <code className="min-w-0 flex-1 truncate rounded-md bg-muted px-2 py-1.5 font-mono text-xs">{enrollment.agent_key}</code>
            <Button variant="outline" size="sm" onClick={() => void copy(enrollment.agent_key)}>
              Copy key
            </Button>
          </div>
          <pre className={pre}>{enrollment.secret_command}</pre>
          <p className="text-xs text-muted-foreground">Run it, paste the key, then press Ctrl-D.</p>
        </section>
        <section className="grid gap-2">
          <h3 className="text-sm font-medium">2. Deploy the agent stack</h3>
          <fieldset className="grid gap-1 text-sm">
            <legend className="sr-only">Where FleetOps runs</legend>
            <label className="flex items-center gap-2">
              <input type="radio" name="swarm-variant" checked={!inside} onChange={() => setInside(false)} />
              FleetOps runs outside the swarm
            </label>
            <label className="flex items-center gap-2">
              <input type="radio" name="swarm-variant" checked={inside} disabled={!enrollment.inside} onChange={() => setInside(true)} />
              FleetOps runs inside the swarm
            </label>
            {enrollment.inside_unavailable && <p className="text-xs text-muted-foreground">{enrollment.inside_unavailable}</p>}
          </fieldset>
          <pre className={pre}>{stack.stack_yaml}</pre>
          <div className="flex gap-2">
            <Button variant="outline" size="sm" onClick={() => void copy(stack.stack_yaml)}>
              Copy stack file
            </Button>
            <Button variant="outline" size="sm" onClick={() => download(stack.stack_yaml)}>
              Download fleetops-agent.yml
            </Button>
          </div>
          <pre className={pre}>{stack.deploy_command}</pre>
          <p className="text-xs text-muted-foreground">
            Each node&apos;s agent shows on the environment&apos;s Nodes page within a few seconds of starting.
          </p>
        </section>
        {rotated && (
          <p className="text-sm text-muted-foreground">
            After every node runs with the new secret, click Finish rotation. The old key stops working in 24 hours anyway.
          </p>
        )}
        <DialogFooter>
          {rotated && (
            <Button variant="outline" onClick={() => void finish()}>
              Finish rotation
            </Button>
          )}
          <Button onClick={onClose}>Done</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
