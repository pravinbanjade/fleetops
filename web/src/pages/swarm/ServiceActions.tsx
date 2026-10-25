import { useQueryClient } from "@tanstack/react-query";
import { RefreshCw, Scaling, Trash2, Undo2 } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import { useEnvAccess } from "@/api/access";
import { redeploySwarmService, removeSwarmService, rollbackSwarmService, scaleSwarmService } from "@/api/gen/sdk.gen";
import type { ServiceSummary } from "@/api/gen/types.gen";
import { swarmKeys } from "@/api/queries";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { RowMenu, type QuickItem } from "@/components/RowMenu";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { errorMessage } from "@/lib/errors";
import { useGuarded } from "./guarded";

type Asking = "scale" | "redeploy" | "rollback" | "remove" | null;

/** A service's ⋮ menu: scale, redeploy, roll back and remove, each guarded. */
export function ServiceActions({ env, service, onRemoved }: { env: string; service: ServiceSummary; onRemoved?: () => void }) {
  const qc = useQueryClient();
  const { can } = useEnvAccess(env);
  const { run, dialog } = useGuarded();
  const [asking, setAsking] = useState<Asking>(null);
  const [replicas, setReplicas] = useState(service.replicas_desired ?? 1);
  const { id, name } = service;
  const path = { env, id };

  const act = (action: (confirm?: string) => Promise<unknown>, done: string, after?: () => void) =>
    run(action, name)
      .then(async (ran) => {
        if (!ran) return;
        toast.success(done);
        await qc.invalidateQueries({ queryKey: swarmKeys.all(env) });
        after?.();
      })
      .catch((e) => toast.error(errorMessage(e)));

  const items: QuickItem[] = [];
  if (can("swarm-services.operate")) {
    if (service.mode === "replicated")
      items.push({
        id: "scale",
        label: "Scale",
        icon: Scaling,
        onSelect: () => {
          setReplicas(service.replicas_desired ?? 1);
          setAsking("scale");
        },
      });
    items.push({ id: "redeploy", label: "Redeploy", icon: RefreshCw, onSelect: () => setAsking("redeploy") });
    items.push({ id: "rollback", label: "Roll back", icon: Undo2, onSelect: () => setAsking("rollback") });
  }
  if (can("swarm-services.delete")) items.push({ id: "remove", label: "Remove", icon: Trash2, danger: true, onSelect: () => setAsking("remove") });

  const close = (open: boolean) => !open && setAsking(null);
  return (
    <>
      <RowMenu label={name} items={items} />
      {asking === "scale" && (
        <ConfirmDialog
          open
          onOpenChange={close}
          title={`Scale ${name}`}
          confirmLabel="Scale"
          destructive={false}
          onConfirm={async () => void act((confirm) => scaleSwarmService({ path, body: { replicas, confirm }, throwOnError: true }), `${name} scaled to ${replicas}`)}
        >
          <div className="grid gap-1.5">
            <Label htmlFor="scale-replicas">Replicas</Label>
            <Input id="scale-replicas" type="number" min={0} value={replicas} onChange={(e) => setReplicas(Math.max(0, Number(e.target.value)))} />
          </div>
        </ConfirmDialog>
      )}
      {asking === "redeploy" && (
        <ConfirmDialog
          open
          onOpenChange={close}
          title={`Redeploy ${name}?`}
          description="Swarm replaces every task with the same spec, following the service's update policy."
          confirmLabel="Redeploy"
          destructive={false}
          onConfirm={async () => void act((confirm) => redeploySwarmService({ path, body: { confirm }, throwOnError: true }), `${name} is redeploying`)}
        />
      )}
      {asking === "rollback" && (
        <ConfirmDialog
          open
          onOpenChange={close}
          title={`Roll back ${name}?`}
          description="Swarm goes back to the service's previous spec."
          confirmLabel="Roll back"
          onConfirm={async () => void act((confirm) => rollbackSwarmService({ path, body: { confirm }, throwOnError: true }), `${name} is rolling back`)}
        />
      )}
      {asking === "remove" && (
        <ConfirmDialog
          open
          onOpenChange={close}
          title={`Remove ${name}?`}
          description="Its tasks stop and the service is gone. Its secrets, configs and networks stay."
          confirmLabel="Remove"
          onConfirm={async () => void act((confirm) => removeSwarmService({ path, query: { confirm }, throwOnError: true }), `${name} removed`, onRemoved)}
        />
      )}
      {dialog}
    </>
  );
}
