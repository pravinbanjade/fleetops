import { useQueryClient } from "@tanstack/react-query";
import { Pause, Play, RotateCw, Square, Zap, type LucideIcon } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import { keys } from "@/api/queries";
import { deleteContainer } from "@/api/gen/sdk.gen";
import type { ContainerActionName } from "@/api/gen/types.gen";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { Checkbox } from "@/components/ui/checkbox";
import { Label } from "@/components/ui/label";
import { bulkMessage, runBulk } from "@/lib/bulk";

/** What each bulk action is called before and after it runs. */
export const ACTIONS: { action: ContainerActionName; label: string; done: string; icon: LucideIcon }[] = [
  { action: "start", label: "Start", done: "Started", icon: Play },
  { action: "stop", label: "Stop", done: "Stopped", icon: Square },
  { action: "restart", label: "Restart", done: "Restarted", icon: RotateCw },
  { action: "kill", label: "Kill", done: "Killed", icon: Zap },
  { action: "pause", label: "Pause", done: "Paused", icon: Pause },
  { action: "unpause", label: "Unpause", done: "Unpaused", icon: Play },
];

/** The actions that make sense in the container's current state. */
export function actionsFor(state: string): ContainerActionName[] {
  switch (state) {
    case "running":
      return ["stop", "restart", "kill", "pause"];
    case "paused":
      return ["unpause", "kill"];
    case "restarting":
      return ["stop", "kill"];
    default:
      return ["start"];
  }
}

/** Removes the chosen containers, with Docker's `force` and `volumes` options. */
export function RemoveDialog({
  env,
  containers,
  onDone,
}: {
  env: string;
  containers: { id: string; name: string }[];
  onDone: (removed: boolean) => void;
}) {
  const qc = useQueryClient();
  const [force, setForce] = useState(false);
  const [volumes, setVolumes] = useState(false);
  const one = containers.length === 1;
  return (
    <ConfirmDialog
      open
      onOpenChange={(open) => !open && onDone(false)}
      title={one ? `Remove ${containers[0].name}?` : `Remove ${containers.length} containers?`}
      description={one ? undefined : containers.map((c) => c.name).join(", ")}
      confirmLabel="Remove"
      onConfirm={async () => {
        const r = await runBulk(containers, (c) => c.name, (c) =>
          deleteContainer({ path: { env, id: c.id }, query: { force, volumes } }),
        );
        await qc.invalidateQueries({ queryKey: keys.containers(env) });
        if (r.ok === 0) throw one ? r.failed[0].error : new Error(bulkMessage("Removed", "container", r));
        const msg = bulkMessage("Removed", "container", r);
        if (r.failed.length) toast.error(msg);
        else toast.success(msg);
        onDone(r.ok > 0);
      }}
    >
      <div className="grid gap-3">
        <div className="flex items-center gap-2">
          <Checkbox id="rm-force" checked={force} onCheckedChange={(v) => setForce(!!v)} />
          <Label htmlFor="rm-force">Force (stop running containers first)</Label>
        </div>
        <div className="flex items-center gap-2">
          <Checkbox id="rm-volumes" checked={volumes} onCheckedChange={(v) => setVolumes(!!v)} />
          <Label htmlFor="rm-volumes">Also remove anonymous volumes</Label>
        </div>
      </div>
    </ConfirmDialog>
  );
}
