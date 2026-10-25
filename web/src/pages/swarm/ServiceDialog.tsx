import { useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { toast } from "sonner";
import { createSwarmService, editSwarmService } from "@/api/gen/sdk.gen";
import type { ServiceDetail } from "@/api/gen/types.gen";
import { swarmKeys } from "@/api/queries";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { NativeSelect } from "@/components/ui/native-select";
import { Textarea } from "@/components/ui/textarea";
import { errorMessage } from "@/lib/errors";
import { FormError } from "@/pages/AuthLayout";
import { useGuarded } from "./guarded";
import { EMPTY_FORM, formFromSpec, specFromForm, type ServiceForm } from "./serviceSpec";

type Target = { mode: "create" } | { mode: "edit"; service: ServiceDetail };

function Field({ id, label, children }: { id: string; label: string; children: React.ReactNode }) {
  return (
    <div className="grid gap-1.5">
      <Label htmlFor={id}>{label}</Label>
      {children}
    </div>
  );
}

/** Creates a service, or edits one, as a form or as Docker's raw spec. */
export function ServiceDialog({ env, target, onClose }: { env: string; target: Target; onClose: () => void }) {
  const qc = useQueryClient();
  const { run, dialog } = useGuarded();
  // The spec and version as the dialog opened: a refetch underneath must
  // not move the version this edit is saved against.
  const [opened] = useState(() => (target.mode === "edit" ? { spec: target.service.spec as Record<string, unknown>, version: target.service.version } : { spec: {}, version: 0 }));
  // What the form edits on top of: the opened spec, or the raw spec last switched from.
  const [base, setBase] = useState<Record<string, unknown>>(opened.spec);
  const [form, setForm] = useState<ServiceForm>(() => (target.mode === "edit" ? formFromSpec(opened.spec) : EMPTY_FORM));
  const [raw, setRaw] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const set = <K extends keyof ServiceForm>(k: K, v: ServiceForm[K]) => setForm((f) => ({ ...f, [k]: v }));

  /** The spec as it stands in whichever view is open; throws on bad input. */
  const current = (): Record<string, unknown> => {
    if (raw === null) return specFromForm(form, base);
    try {
      return JSON.parse(raw);
    } catch (e) {
      throw new Error(`the spec is not valid JSON: ${(e as Error).message}`, { cause: e });
    }
  };

  const toRaw = () => {
    try {
      setRaw(JSON.stringify(specFromForm(form, base), null, 2));
      setError(null);
    } catch (e) {
      setError(errorMessage(e));
    }
  };
  const toForm = () => {
    try {
      const spec = current();
      setBase(spec);
      setForm(formFromSpec(spec));
      setRaw(null);
      setError(null);
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  const save = async () => {
    setError(null);
    let spec: Record<string, unknown>;
    try {
      spec = current();
    } catch (e) {
      setError(errorMessage(e));
      return;
    }
    setPending(true);
    try {
      if (target.mode === "create") {
        await createSwarmService({ path: { env }, body: { spec }, throwOnError: true });
        toast.success(`${String(spec.Name)} created`);
      } else {
        const { id, name } = target.service;
        const version = opened.version;
        const ran = await run((confirm) => editSwarmService({ path: { env, id }, body: { spec, version, confirm }, throwOnError: true }), name);
        if (!ran) return;
        toast.success(`${name} updated`);
      }
      await qc.invalidateQueries({ queryKey: swarmKeys.all(env) });
      onClose();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setPending(false);
    }
  };

  const editing = target.mode === "edit";
  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>{editing ? `Edit ${target.service.name}` : "Create service"}</DialogTitle>
          <DialogDescription>
            {editing ? "Saving updates the service; Swarm replaces its tasks as the update policy says." : "The image must come from a registry every node can pull from."}
          </DialogDescription>
        </DialogHeader>
        <div className="flex gap-2">
          <Button size="sm" variant={raw === null ? "default" : "outline"} onClick={toForm}>
            Form
          </Button>
          <Button size="sm" variant={raw !== null ? "default" : "outline"} onClick={toRaw}>
            Raw spec
          </Button>
        </div>
        {raw === null ? (
          <div className="grid gap-3">
            <Field id="svc-name" label="Name">
              <Input id="svc-name" value={form.name} disabled={editing} onChange={(e) => set("name", e.target.value)} />
            </Field>
            <Field id="svc-image" label="Image">
              <Input id="svc-image" className="font-mono" value={form.image} onChange={(e) => set("image", e.target.value)} />
            </Field>
            <div className="grid grid-cols-2 gap-3">
              <Field id="svc-mode" label="Mode">
                <NativeSelect id="svc-mode" value={form.mode} onChange={(e) => set("mode", e.target.value as ServiceForm["mode"])}>
                  <option value="replicated">Replicated</option>
                  <option value="global">Global (one per node)</option>
                </NativeSelect>
              </Field>
              {form.mode === "replicated" && (
                <Field id="svc-replicas" label="Replicas">
                  <Input id="svc-replicas" type="number" min={0} value={form.replicas} onChange={(e) => set("replicas", Math.max(0, Number(e.target.value)))} />
                </Field>
              )}
            </div>
            <Field id="svc-ports" label="Ports (published:target[/udp], one per line)">
              <Textarea id="svc-ports" className="font-mono text-xs" rows={2} value={form.ports} onChange={(e) => set("ports", e.target.value)} />
            </Field>
            <Field id="svc-env" label="Environment (KEY=value, one per line)">
              <Textarea id="svc-env" className="font-mono text-xs" rows={3} value={form.env} onChange={(e) => set("env", e.target.value)} />
            </Field>
            <Field id="svc-constraints" label="Placement constraints (one per line)">
              <Textarea id="svc-constraints" className="font-mono text-xs" rows={2} value={form.constraints} onChange={(e) => set("constraints", e.target.value)} />
            </Field>
          </div>
        ) : (
          <Field id="svc-raw" label="ServiceSpec (JSON)">
            <Textarea id="svc-raw" className="font-mono text-xs" rows={18} value={raw} onChange={(e) => setRaw(e.target.value)} />
          </Field>
        )}
        <FormError message={error} />
        <DialogFooter>
          <Button variant="outline" onClick={onClose}>
            Cancel
          </Button>
          <Button disabled={pending} onClick={() => void save()}>
            {editing ? "Save" : "Create"}
          </Button>
        </DialogFooter>
        {dialog}
      </DialogContent>
    </Dialog>
  );
}
