import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { useState, type FormEvent } from "react";
import { toast } from "sonner";
import { keys } from "@/api/queries";
import { createNetwork, deleteNetwork, pruneNetworks } from "@/api/gen/sdk.gen";
import type { NetworkSpec } from "@/api/gen/types.gen";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { bulkMessage, runBulk } from "@/lib/bulk";
import { errorMessage } from "@/lib/errors";
import { parseKeyValues } from "@/lib/kv";
import { FormError } from "@/pages/AuthLayout";

/** Docker's own networks, which cannot be removed. */
export const BUILTIN_NETWORKS = ["bridge", "host", "none"];

export function RemoveNetworksDialog({
  env,
  networks,
  onDone,
}: {
  env: string;
  networks: { id: string; name: string }[];
  onDone: (removed: boolean) => void;
}) {
  const qc = useQueryClient();
  const one = networks.length === 1;
  return (
    <ConfirmDialog
      open
      onOpenChange={(open) => !open && onDone(false)}
      title={one ? `Remove network ${networks[0].name}?` : `Remove ${networks.length} networks?`}
      description={one ? undefined : networks.map((n) => n.name).join(", ")}
      confirmLabel="Remove"
      onConfirm={async () => {
        const r = await runBulk(networks, (n) => n.name, (n) => deleteNetwork({ path: { env, id: n.id } }));
        await qc.invalidateQueries({ queryKey: keys.networks(env) });
        if (r.ok === 0) throw one ? r.failed[0].error : new Error(bulkMessage("Removed", "network", r));
        const msg = bulkMessage("Removed", "network", r);
        if (r.failed.length) toast.error(msg);
        else toast.success(msg);
        onDone(true);
      }}
    />
  );
}

export function PruneNetworksDialog({ env, onDone }: { env: string; onDone: () => void }) {
  const qc = useQueryClient();
  return (
    <ConfirmDialog
      open
      onOpenChange={(open) => !open && onDone()}
      title="Prune networks?"
      description="Removes every network no container is attached to. Docker's own networks are kept."
      confirmLabel="Prune"
      onConfirm={async () => {
        const { data } = await pruneNetworks({ path: { env } });
        await qc.invalidateQueries({ queryKey: keys.networks(env) });
        toast.success(`Removed ${data.deleted.length} networks`);
        onDone();
      }}
    />
  );
}

export function CreateNetworkDialog({ env, onClose }: { env: string; onClose: () => void }) {
  const qc = useQueryClient();
  const navigate = useNavigate();
  const [f, setF] = useState({ name: "", driver: "bridge", subnet: "", gateway: "", labels: "" });
  const [internal, setInternal] = useState(false);
  const [attachable, setAttachable] = useState(false);
  const [invalid, setInvalid] = useState<string | null>(null);
  const set = (k: keyof typeof f) => (e: { target: { value: string } }) => setF({ ...f, [k]: e.target.value });
  const m = useMutation({
    meta: { inline: true },
    mutationFn: (body: NetworkSpec) => createNetwork({ path: { env }, body }),
    onSuccess: async ({ data }) => {
      await qc.invalidateQueries({ queryKey: keys.networks(env) });
      toast.success(`Created network ${f.name}`);
      onClose();
      void navigate({ to: "/envs/$env/networks/$id", params: { env, id: data.id } });
    },
  });
  const submit = (e: FormEvent) => {
    e.preventDefault();
    try {
      if (f.gateway.trim() && !f.subnet.trim()) throw new Error("A gateway needs a subnet");
      const body: NetworkSpec = {
        name: f.name.trim(),
        driver: f.driver.trim() || null,
        subnet: f.subnet.trim() || null,
        gateway: f.gateway.trim() || null,
        internal,
        attachable,
        labels: parseKeyValues(f.labels, "Label"),
      };
      setInvalid(null);
      m.mutate(body);
    } catch (err) {
      setInvalid((err as Error).message);
    }
  };
  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Create network</DialogTitle>
        </DialogHeader>
        <form onSubmit={submit} className="grid gap-4">
          <div className="grid gap-2">
            <Label htmlFor="net-name">Name</Label>
            <Input id="net-name" required value={f.name} onChange={set("name")} />
          </div>
          <div className="grid gap-2">
            <Label htmlFor="net-driver">Driver</Label>
            <Input id="net-driver" value={f.driver} onChange={set("driver")} />
          </div>
          <div className="grid gap-4 sm:grid-cols-2">
            <div className="grid gap-2">
              <Label htmlFor="net-subnet">Subnet</Label>
              <Input id="net-subnet" placeholder="172.30.0.0/16" value={f.subnet} onChange={set("subnet")} />
            </div>
            <div className="grid gap-2">
              <Label htmlFor="net-gateway">Gateway</Label>
              <Input id="net-gateway" placeholder="172.30.0.1" value={f.gateway} onChange={set("gateway")} />
            </div>
          </div>
          <div className="grid gap-2">
            <Label htmlFor="net-labels">Labels</Label>
            <Textarea id="net-labels" className="font-mono" value={f.labels} onChange={set("labels")} />
          </div>
          <div className="flex items-center gap-2">
            <Checkbox id="net-internal" checked={internal} onCheckedChange={(v) => setInternal(!!v)} />
            <Label htmlFor="net-internal">Internal (no access to outside networks)</Label>
          </div>
          <div className="flex items-center gap-2">
            <Checkbox id="net-attachable" checked={attachable} onCheckedChange={(v) => setAttachable(!!v)} />
            <Label htmlFor="net-attachable">Attachable</Label>
          </div>
          <FormError message={invalid ?? (m.error ? errorMessage(m.error) : null)} />
          <DialogFooter>
            <Button type="submit" disabled={m.isPending}>
              Create
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
