import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { useState } from "react";
import { deleteKube, restartKube, scaleKube } from "@/api/gen/sdk.gen";
import type { KubeKind } from "@/api/gen/types.gen";
import { keys } from "@/api/queries";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { Button } from "@/components/ui/button";
import { FormDialog } from "@/pages/admin/FormDialog";
import { KIND_LABEL, RESTARTABLE, SCALABLE } from "./kinds";

export function KubeActions({ env, kind, ns, name, replicas }: { env: string; kind: KubeKind; ns: string; name: string; replicas?: number }) {
  const qc = useQueryClient();
  const navigate = useNavigate();
  const [open, setOpen] = useState<"scale" | "restart" | "delete" | null>(null);
  const [count, setCount] = useState(String(replicas ?? 1));
  const path = { env, kind, ns, name };
  const refresh = () => qc.invalidateQueries({ queryKey: keys.kube(env, kind).slice(0, 4) });
  const restart = useMutation({ mutationFn: () => restartKube({ path, throwOnError: true }), onSuccess: refresh });
  const remove = useMutation({
    mutationFn: () => deleteKube({ path, throwOnError: true }),
    onSuccess: async () => {
      await refresh();
      void navigate({ to: "/envs/$env/k8s/$kind", params: { env, kind } });
    },
  });
  const label = KIND_LABEL[kind].replace(/s$/, "");
  return (
    <div className="flex gap-2">
      {SCALABLE.includes(kind) && <Button variant="outline" size="sm" onClick={() => {
            setCount(String(replicas ?? 1));
            setOpen("scale");
          }}>Scale</Button>}
      {RESTARTABLE.includes(kind) && <Button variant="outline" size="sm" onClick={() => setOpen("restart")}>Restart</Button>}
      <Button variant="outline" size="sm" onClick={() => setOpen("delete")}>Delete</Button>
      {open === "scale" && (
        <FormDialog
          title={`Scale ${name}`}
          submitLabel="Scale"
          onClose={() => setOpen(null)}
          onSubmit={async () => {
            const n = Number(count);
            if (count.trim() === "" || !Number.isInteger(n) || n < 0) {
              throw new Error("Replicas must be a whole number, 0 or more");
            }
            await scaleKube({ path, body: { replicas: n }, throwOnError: true });
            await refresh();
          }}
        >
          <label className="grid gap-1 text-sm">
            Replicas
            <input type="number" required min={0} step={1} className="h-9 rounded-md border px-2" value={count} onChange={(e) => setCount(e.target.value)} />
          </label>
        </FormDialog>
      )}
      {open === "restart" && (
        <ConfirmDialog
          open
          onOpenChange={(o) => !o && setOpen(null)}
          title={`Restart ${name}?`}
          description="Its pods are replaced one by one, as kubectl rollout restart does."
          confirmLabel="Restart"
          onConfirm={() => restart.mutateAsync()}
        />
      )}
      {open === "delete" && (
        <ConfirmDialog
          open
          onOpenChange={(o) => !o && setOpen(null)}
          title={`Delete ${label} ${name}?`}
          description={kind === "namespaces" ? "Everything in the namespace is deleted with it." : "This cannot be undone."}
          confirmLabel="Delete"
          destructive
          onConfirm={() => remove.mutateAsync()}
        />
      )}
    </div>
  );
}
