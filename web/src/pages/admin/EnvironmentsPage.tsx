import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { toast } from "sonner";
import { envsQuery, keys } from "@/api/queries";
import { createEnv, deleteEnv, updateEnv } from "@/api/gen/sdk.gen";
import type { EnvironmentRecord } from "@/api/gen/types.gen";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { columns, DataTable, type Column } from "@/components/DataTable";
import { Page } from "@/components/Page";
import { EnvStatusBadge } from "@/components/Shell";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { FormDialog } from "./FormDialog";

type Dialog = { kind: "add" } | { kind: "edit" | "delete"; env: EnvironmentRecord } | null;

const col = columns<EnvironmentRecord>();

export function EnvironmentsPage() {
  const qc = useQueryClient();
  const envs = useQuery(envsQuery);
  const [dialog, setDialog] = useState<Dialog>(null);
  const close = () => setDialog(null);
  // Renaming or removing one changes every environment-scoped key, so all are refetched.
  const refresh = () => Promise.all([qc.invalidateQueries({ queryKey: keys.envs }), qc.invalidateQueries({ queryKey: ["env"] })]);
  const [cols] = useState<Column<EnvironmentRecord>[]>(() => [
    col.accessor("name", { header: "Name", cell: (c) => <span className="font-medium">{c.getValue()}</span> }),
    col.display({ id: "status", header: "Status", cell: (c) => <EnvStatusBadge env={c.row.original.name} /> }),
    col.accessor("kind", { header: "Kind" }),
    col.accessor("url", { header: "URL", cell: (c) => <code className="text-xs">{c.getValue()}</code> }),
    col.display({
      id: "actions",
      header: () => <span className="sr-only">Actions</span>,
      cell: (c) => (
        <span className="flex justify-end gap-2">
          <Button variant="outline" size="sm" onClick={() => setDialog({ kind: "edit", env: c.row.original })}>
            Edit
          </Button>
          <Button variant="outline" size="sm" onClick={() => setDialog({ kind: "delete", env: c.row.original })}>
            Delete
          </Button>
        </span>
      ),
    }),
  ]);
  return (
    <Page title="Environments" actions={<Button onClick={() => setDialog({ kind: "add" })}>Add environment</Button>}>
      <DataTable
        label="Environments"
        data={envs.data}
        error={envs.error}
        columns={cols}
        getRowId={(e) => String(e.id)}
        empty="No environments."
        initialSorting={[{ id: "name", desc: false }]}
      />
      {dialog?.kind === "add" && <EnvForm onClose={close} onDone={refresh} />}
      {dialog?.kind === "edit" && <EnvForm env={dialog.env} onClose={close} onDone={refresh} />}
      {dialog?.kind === "delete" && (
        <ConfirmDialog
          open
          onOpenChange={(open) => !open && close()}
          title={`Delete environment ${dialog.env.name}?`}
          description="FleetOps stops managing it. Its containers and other resources are not touched."
          confirmLabel="Delete"
          onConfirm={async () => {
            await deleteEnv({ path: { env: dialog.env.name } });
            toast.success(`Deleted ${dialog.env.name}`);
            await refresh();
          }}
        />
      )}
    </Page>
  );
}

function EnvForm({ env, onClose, onDone }: { env?: EnvironmentRecord; onClose: () => void; onDone: () => Promise<unknown> }) {
  const [name, setName] = useState(env?.name ?? "");
  const [url, setUrl] = useState(env?.url ?? "unix:///var/run/docker.sock");
  return (
    <FormDialog
      title={env ? `Edit ${env.name}` : "Add environment"}
      description="A Docker engine this server can reach: a unix socket, or tcp:// / http(s):// for an engine exposing its API."
      submitLabel={env ? "Save" : "Add"}
      onClose={onClose}
      onSubmit={async () => {
        if (env) {
          const body = { name: name.trim() === env.name ? null : name.trim(), url: url.trim() === env.url ? null : url.trim() };
          if (!body.name && !body.url) return;
          await updateEnv({ path: { env: env.name }, body });
        } else {
          await createEnv({ body: { name: name.trim(), url: url.trim(), kind: "docker-local" } });
        }
        await onDone();
      }}
    >
      <div className="grid gap-2">
        <Label htmlFor="env-name">Name</Label>
        <Input id="env-name" required value={name} onChange={(e) => setName(e.target.value)} />
      </div>
      <div className="grid gap-2">
        <Label htmlFor="env-url">Docker URL</Label>
        <Input id="env-url" required className="font-mono" value={url} onChange={(e) => setUrl(e.target.value)} />
      </div>
    </FormDialog>
  );
}
