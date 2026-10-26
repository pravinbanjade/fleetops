import { Plus } from "lucide-react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { useState, type FormEvent } from "react";
import { toast } from "sonner";
import { keys } from "@/api/queries";
import { createVolume, deleteVolume, pruneVolumes } from "@/api/gen/sdk.gen";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { bulkMessage, runBulk } from "@/lib/bulk";
import { errorMessage } from "@/lib/errors";
import { formatBytes } from "@/lib/format";
import { parseKeyValues } from "@/lib/kv";
import { FormError } from "@/pages/AuthLayout";

export function RemoveVolumesDialog({
  env,
  names,
  onDone,
}: {
  env: string;
  names: string[];
  onDone: (removed: boolean) => void;
}) {
  const qc = useQueryClient();
  const [force, setForce] = useState(false);
  const one = names.length === 1;
  return (
    <ConfirmDialog
      open
      onOpenChange={(open) => !open && onDone(false)}
      title={one ? `Remove volume ${names[0]}?` : `Remove ${names.length} volumes?`}
      description={
        <>
          The data in {one ? "it" : "them"} is deleted for good.{one ? "" : ` ${names.join(", ")}`}
        </>
      }
      confirmLabel="Remove"
      onConfirm={async () => {
        const r = await runBulk(names, (n) => n, (name) => deleteVolume({ path: { env, name }, query: { force } }));
        await qc.invalidateQueries({ queryKey: keys.volumes(env) });
        if (r.ok === 0) throw one ? r.failed[0].error : new Error(bulkMessage("Removed", "volume", r));
        const msg = bulkMessage("Removed", "volume", r);
        if (r.failed.length) toast.error(msg);
        else toast.success(msg);
        onDone(true);
      }}
    >
      <div className="flex items-center gap-2">
        <Checkbox id="vol-force" checked={force} onCheckedChange={(v) => setForce(!!v)} />
        <Label htmlFor="vol-force">Force (remove even if the driver reports it in use)</Label>
      </div>
    </ConfirmDialog>
  );
}

export function PruneVolumesDialog({ env, onDone }: { env: string; onDone: () => void }) {
  const qc = useQueryClient();
  return (
    <ConfirmDialog
      open
      onOpenChange={(open) => !open && onDone()}
      title="Prune volumes?"
      description="Removes anonymous volumes no container uses, and their data. Named volumes are kept."
      confirmLabel="Prune"
      onConfirm={async () => {
        const { data } = await pruneVolumes({ path: { env } });
        await qc.invalidateQueries({ queryKey: keys.volumes(env) });
        toast.success(`Removed ${data.deleted.length} volumes, reclaimed ${formatBytes(data.space_reclaimed)}`);
        onDone();
      }}
    />
  );
}

export function CreateVolumeDialog({ env, onClose }: { env: string; onClose: () => void }) {
  const qc = useQueryClient();
  const navigate = useNavigate();
  const [name, setName] = useState("");
  const [driver, setDriver] = useState("local");
  const [opts, setOpts] = useState("");
  const [labels, setLabels] = useState("");
  const [invalid, setInvalid] = useState<string | null>(null);
  const m = useMutation({
    meta: { inline: true },
    mutationFn: (body: Parameters<typeof createVolume>[0]["body"]) => createVolume({ path: { env }, body }),
    onSuccess: async ({ data }) => {
      await qc.invalidateQueries({ queryKey: keys.volumes(env) });
      toast.success(`Created volume ${data.name}`);
      onClose();
      void navigate({ to: "/envs/$env/volumes/$name", params: { env, name: data.name } });
    },
  });
  const submit = (e: FormEvent) => {
    e.preventDefault();
    try {
      const body = {
        name: name.trim() || undefined,
        driver: driver.trim() || null,
        driver_opts: parseKeyValues(opts, "Option"),
        labels: parseKeyValues(labels, "Label"),
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
          <DialogTitle>Create volume</DialogTitle>
        </DialogHeader>
        <form onSubmit={submit} className="grid gap-4">
          <div className="grid gap-2">
            <Label htmlFor="vol-name">Name</Label>
            <Input id="vol-name" placeholder="Leave empty for a generated name" value={name} onChange={(e) => setName(e.target.value)} />
          </div>
          <div className="grid gap-2">
            <Label htmlFor="vol-driver">Driver</Label>
            <Input id="vol-driver" value={driver} onChange={(e) => setDriver(e.target.value)} />
          </div>
          <div className="grid gap-2">
            <Label htmlFor="vol-opts">Driver options</Label>
            <Textarea
              id="vol-opts"
              className="font-mono"
              placeholder={"type=nfs\no=addr=10.0.0.5,rw"}
              value={opts}
              onChange={(e) => setOpts(e.target.value)}
            />
            <p className="text-xs text-muted-foreground">One key=value per line.</p>
          </div>
          <div className="grid gap-2">
            <Label htmlFor="vol-labels">Labels</Label>
            <Textarea id="vol-labels" className="font-mono" value={labels} onChange={(e) => setLabels(e.target.value)} />
          </div>
          <FormError message={invalid ?? (m.error ? errorMessage(m.error) : null)} />
          <DialogFooter>
            <Button type="submit" disabled={m.isPending}>
              <Plus aria-hidden />
              Create
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
