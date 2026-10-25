import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { useState } from "react";
import { removeStack } from "@/api/gen/sdk.gen";
import { keys, swarmKeys, volumesQuery } from "@/api/queries";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";

/**
 * Removes a stack; its volumes go only when ticked, named, and the stack name
 * typed. A Swarm stack's volumes live on its nodes and always stay.
 */
export function RemoveStackDialog({
  env,
  stack,
  swarm = false,
  onClose,
}: {
  env: string;
  stack: string;
  swarm?: boolean;
  onClose: () => void;
}) {
  const qc = useQueryClient();
  const navigate = useNavigate();
  const [volumes, setVolumes] = useState(false);
  const [typed, setTyped] = useState("");
  const vols = useQuery({ ...volumesQuery(env), enabled: !swarm });
  const ours = (vols.data ?? []).filter((v) => v.labels["com.docker.compose.project"] === stack).map((v) => v.name);
  const ready = !volumes || typed === stack;
  return (
    <ConfirmDialog
      open
      onOpenChange={(o) => !o && onClose()}
      title={`Remove ${stack}?`}
      description={
        swarm
          ? "Removes the stack's services, networks, secrets and configs, and its revisions in FleetOps. Volumes stay on the nodes."
          : "Removes the stack's containers and networks, and its revisions in FleetOps."
      }
      confirmLabel="Remove"
      onConfirm={async () => {
        if (!ready) throw new Error(`Type ${stack} to remove its volumes`);
        await removeStack({ path: { env, stack }, query: { volumes } });
        await qc.invalidateQueries({ queryKey: keys.stacks(env) });
        await qc.invalidateQueries({ queryKey: swarm ? swarmKeys.all(env) : keys.containers(env) });
        await navigate({ to: "/envs/$env/stacks", params: { env } });
      }}
    >
      {!swarm && (
      <div className="grid gap-3 text-sm">
        <Label className="flex items-center gap-2 font-normal">
          <Checkbox checked={volumes} onCheckedChange={(c) => setVolumes(c === true)} />
          Also remove volumes
        </Label>
        {volumes && (
          <>
            <p>
              {ours.length ? (
                <>
                  These volumes and their data are deleted: <span className="font-ident">{ours.join(", ")}</span>
                </>
              ) : (
                "The stack has no volumes."
              )}
            </p>
            <Label htmlFor="remove-confirm">Type {stack} to confirm</Label>
            <Input id="remove-confirm" className="font-ident" value={typed} onChange={(e) => setTyped(e.target.value)} />
          </>
        )}
      </div>
      )}
    </ConfirmDialog>
  );
}
