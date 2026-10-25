import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Eye, Trash2 } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import { useEnvAccess } from "@/api/access";
import {
  createSwarmConfig,
  createSwarmNetwork,
  createSwarmSecret,
  getSwarmConfigData,
  removeSwarmConfig,
  removeSwarmNetwork,
  removeSwarmSecret,
} from "@/api/gen/sdk.gen";
import { swarmKeys } from "@/api/queries";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { RowMenu, type QuickItem } from "@/components/RowMenu";
import { Checkbox } from "@/components/ui/checkbox";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { errorMessage } from "@/lib/errors";

export type ObjectKind = "secret" | "config" | "network";

const ROW = { secret: "swarm-secrets", config: "swarm-configs", network: "networks" } as const;

/** The permission for `verb` on `kind`. */
export const permOf = (kind: ObjectKind, verb: "create" | "delete" | "reveal") => `${ROW[kind]}.${verb}`;

/** Creates a secret or config: a name and a value. A secret's value is write-only. */
export function CreateObjectDialog({ env, kind, onClose }: { env: string; kind: "secret" | "config"; onClose: () => void }) {
  const qc = useQueryClient();
  const [name, setName] = useState("");
  const [value, setValue] = useState("");
  const create = kind === "secret" ? createSwarmSecret : createSwarmConfig;
  return (
    <ConfirmDialog
      open
      onOpenChange={(open) => !open && onClose()}
      title={kind === "secret" ? "Create secret" : "Create config"}
      description={kind === "secret" ? "The value is write-only: neither Docker nor FleetOps shows it again." : "Services mount it as a file."}
      confirmLabel="Create"
      destructive={false}
      onConfirm={async () => {
        await create({ path: { env }, body: { name, value }, throwOnError: true });
        setValue("");
        toast.success(`${name} created`);
        await qc.invalidateQueries({ queryKey: swarmKeys.all(env) });
      }}
    >
      <div className="grid gap-3">
        <div className="grid gap-1.5">
          <Label htmlFor="obj-name">Name</Label>
          <Input id="obj-name" value={name} onChange={(e) => setName(e.target.value)} />
        </div>
        <div className="grid gap-1.5">
          <Label htmlFor="obj-value">Value</Label>
          <Textarea id="obj-value" className="font-mono text-xs" rows={6} autoComplete="off" value={value} onChange={(e) => setValue(e.target.value)} />
        </div>
      </div>
    </ConfirmDialog>
  );
}

/** A config's content, fetched (and audited) only when opened. */
export function ConfigContentDialog({ env, id, name, onClose }: { env: string; id: string; name: string; onClose: () => void }) {
  const q = useQuery({
    queryKey: ["env", env, "swarm", "configs", id, "data"],
    queryFn: async () => (await getSwarmConfigData({ path: { env, id }, throwOnError: true })).data,
    gcTime: 0,
  });
  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>{name}</DialogTitle>
        </DialogHeader>
        {q.error && <p role="alert" className="text-sm text-destructive">{errorMessage(q.error)}</p>}
        {q.data &&
          (q.data.text === null || q.data.text === undefined ? (
            <p className="text-sm text-muted-foreground">Binary content ({q.data.size} bytes)</p>
          ) : (
            <pre className="max-h-[60vh] overflow-auto rounded-md bg-muted p-3 font-mono text-xs">{q.data.text}</pre>
          ))}
      </DialogContent>
    </Dialog>
  );
}

/** Creates an overlay network. */
export function CreateSwarmNetworkDialog({ env, onClose }: { env: string; onClose: () => void }) {
  const qc = useQueryClient();
  const [name, setName] = useState("");
  const [subnet, setSubnet] = useState("");
  const [flags, setFlags] = useState({ attachable: false, internal: false, encrypted: false });
  const flag = (k: keyof typeof flags, label: string) => (
    <label className="flex items-center gap-2 text-sm">
      <Checkbox checked={flags[k]} onCheckedChange={(v) => setFlags((f) => ({ ...f, [k]: v === true }))} aria-label={label} />
      {label}
    </label>
  );
  return (
    <ConfirmDialog
      open
      onOpenChange={(open) => !open && onClose()}
      title="Create network"
      description="An overlay network spanning the swarm."
      confirmLabel="Create"
      destructive={false}
      onConfirm={async () => {
        await createSwarmNetwork({ path: { env }, body: { name, subnet: subnet.trim() || null, ...flags }, throwOnError: true });
        toast.success(`${name} created`);
        await qc.invalidateQueries({ queryKey: swarmKeys.all(env) });
      }}
    >
      <div className="grid gap-3">
        <div className="grid gap-1.5">
          <Label htmlFor="net-name">Name</Label>
          <Input id="net-name" value={name} onChange={(e) => setName(e.target.value)} />
        </div>
        <div className="grid gap-1.5">
          <Label htmlFor="net-subnet">Subnet (optional)</Label>
          <Input id="net-subnet" placeholder="10.20.0.0/24" value={subnet} onChange={(e) => setSubnet(e.target.value)} />
        </div>
        {flag("attachable", "Attachable (standalone containers may join)")}
        {flag("internal", "Internal (no route outside)")}
        {flag("encrypted", "Encrypted (IPsec between nodes)")}
      </div>
    </ConfirmDialog>
  );
}

/** A secret's, config's or network's ⋮ menu: view content (configs) and remove. */
export function ObjectActions({
  env,
  kind,
  id,
  name,
  stack = null,
}: {
  env: string;
  kind: ObjectKind;
  id: string;
  name: string;
  /** Its stack; a grant limited to stacks acts on theirs only. */
  stack?: string | null;
}) {
  const qc = useQueryClient();
  const { can } = useEnvAccess(env);
  const [asking, setAsking] = useState<"view" | "remove" | null>(null);
  const items: QuickItem[] = [];
  if (kind === "config" && can(permOf(kind, "reveal"), stack)) items.push({ id: "view", label: "View content", icon: Eye, onSelect: () => setAsking("view") });
  if (can(permOf(kind, "delete"), stack)) items.push({ id: "remove", label: "Remove", icon: Trash2, danger: true, onSelect: () => setAsking("remove") });
  const remove = { secret: removeSwarmSecret, config: removeSwarmConfig, network: removeSwarmNetwork }[kind];
  return (
    <>
      <RowMenu label={name} items={items} />
      {asking === "view" && <ConfigContentDialog env={env} id={id} name={name} onClose={() => setAsking(null)} />}
      {asking === "remove" && (
        <ConfirmDialog
          open
          onOpenChange={(open) => !open && setAsking(null)}
          title={`Remove ${name}?`}
          description="Docker refuses while a service still uses it."
          confirmLabel="Remove"
          onConfirm={async () => {
            await remove({ path: { env, id }, throwOnError: true });
            toast.success(`${name} removed`);
            await qc.invalidateQueries({ queryKey: swarmKeys.all(env) });
          }}
        />
      )}
    </>
  );
}
