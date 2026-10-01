import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { toast } from "sonner";
import { envsQuery, keys } from "@/api/queries";
import { createEnv, deleteEnv, rotateAgent, updateEnv } from "@/api/gen/sdk.gen";
import type { Enrollment, EnvView } from "@/api/gen/types.gen";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { columns, DataTable, type Column } from "@/components/DataTable";
import { Page } from "@/components/Page";
import { EnvStatusBadge } from "@/components/Shell";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { formatAgo } from "@/lib/format";
import { AgentEnrollment } from "./AgentEnrollment";
import { FormDialog } from "./FormDialog";

type Dialog = { kind: "add" } | { kind: "edit" | "delete" | "rotate"; env: EnvView } | null;

const col = columns<EnvView>();

const isAgent = (e: EnvView) => e.kind === "docker-agent";

/** What an agent environment's agent is doing, for the list. */
function AgentCell({ env }: { env: EnvView }) {
  const a = env.agent;
  if (!isAgent(env) || !a) return null;
  if (!a.enrolled) return <span className="text-muted-foreground">waiting for agent</span>;
  return (
    <span className="grid gap-0.5">
      <span className="font-mono text-xs">{a.agent_version}</span>
      {!a.connected && a.last_seen_at != null && <span className="text-xs text-muted-foreground">last seen {formatAgo(a.last_seen_at)}</span>}
      {a.last_error && <span className="text-xs text-destructive">{a.last_error}</span>}
    </span>
  );
}

export function EnvironmentsPage() {
  const qc = useQueryClient();
  const envs = useQuery(envsQuery);
  const [dialog, setDialog] = useState<Dialog>(null);
  // Separate from `dialog`: a confirm dialog closes itself after it succeeds.
  const [enrollment, setEnrollment] = useState<Enrollment | null>(null);
  const close = () => setDialog(null);
  // Renaming or removing one changes every environment-scoped key, so all are refetched.
  const refresh = () => Promise.all([qc.invalidateQueries({ queryKey: keys.envs }), qc.invalidateQueries({ queryKey: ["env"] })]);
  const [cols] = useState<Column<EnvView>[]>(() => [
    col.accessor("name", { header: "Name", cell: (c) => <span className="font-medium">{c.getValue()}</span> }),
    col.display({ id: "status", header: "Status", cell: (c) => <EnvStatusBadge env={c.row.original.name} /> }),
    col.display({ id: "agent", header: "Agent", cell: (c) => <AgentCell env={c.row.original} /> }),
    col.accessor("kind", { header: "Kind" }),
    col.accessor("url", {
      header: "URL",
      cell: (c) => (
        <code className="block max-w-[16rem] truncate text-xs" title={c.getValue()}>
          {c.getValue()}
        </code>
      ),
    }),
    col.display({
      id: "actions",
      header: () => <span className="sr-only">Actions</span>,
      cell: (c) => (
        <span className="flex justify-end gap-2">
          {isAgent(c.row.original) && (
            <Button variant="outline" size="sm" onClick={() => setDialog({ kind: "rotate", env: c.row.original })}>
              Rotate key
            </Button>
          )}
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
      {dialog?.kind === "add" && <EnvForm onClose={close} onDone={refresh} onEnrollment={setEnrollment} />}
      {dialog?.kind === "edit" && <EnvForm env={dialog.env} onClose={close} onDone={refresh} />}
      {dialog?.kind === "rotate" && (
        <ConfirmDialog
          open
          onOpenChange={(open) => !open && close()}
          title={`Rotate key for ${dialog.env.name}?`}
          description="This disconnects the running agent. Run the new command on the host to reconnect it."
          confirmLabel="Rotate"
          onConfirm={async () => {
            const { data } = await rotateAgent({ path: { env: dialog.env.name }, throwOnError: true });
            setEnrollment(data);
            await refresh();
          }}
        />
      )}
      {enrollment && <AgentEnrollment enrollment={enrollment} onClose={() => setEnrollment(null)} />}
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

function EnvForm({
  env,
  onClose,
  onDone,
  onEnrollment,
}: {
  env?: EnvView;
  onClose: () => void;
  onDone: () => Promise<unknown>;
  onEnrollment?: (e: Enrollment) => void;
}) {
  const [name, setName] = useState(env?.name ?? "");
  const [url, setUrl] = useState(env?.url ?? "unix:///var/run/docker.sock");
  const [kind, setKind] = useState<"docker-local" | "docker-agent">(env && isAgent(env) ? "docker-agent" : "docker-local");
  const agent = kind === "docker-agent";
  return (
    <FormDialog
      title={env ? `Edit ${env.name}` : "Add environment"}
      description={
        agent
          ? "A remote host running fleetops-agent, which connects out to this server. You get the command to run there next."
          : "A Docker engine this server can reach: a unix socket, or tcp:// / http(s):// for an engine exposing its API."
      }
      submitLabel={env ? "Save" : "Add"}
      onClose={onClose}
      onSubmit={async () => {
        if (env) {
          const body = {
            name: name.trim() === env.name ? null : name.trim(),
            url: agent || url.trim() === env.url ? null : url.trim(),
          };
          if (!body.name && !body.url) return;
          await updateEnv({ path: { env: env.name }, body });
        } else if (agent) {
          const { data } = await createEnv({ body: { name: name.trim(), kind }, throwOnError: true });
          if (data.enrollment) onEnrollment?.(data.enrollment);
        } else {
          await createEnv({ body: { name: name.trim(), url: url.trim(), kind } });
        }
        await onDone();
      }}
    >
      {!env && (
        <fieldset className="grid gap-2">
          <legend className="mb-2 text-sm font-medium">Kind</legend>
          <label className="flex items-center gap-2 text-sm">
            <input type="radio" name="env-kind" checked={!agent} onChange={() => setKind("docker-local")} />
            Local socket or Docker API URL
          </label>
          <label className="flex items-center gap-2 text-sm">
            <input type="radio" name="env-kind" checked={agent} onChange={() => setKind("docker-agent")} />
            Agent on a remote host
          </label>
        </fieldset>
      )}
      <div className="grid gap-2">
        <Label htmlFor="env-name">Name</Label>
        <Input id="env-name" required value={name} onChange={(e) => setName(e.target.value)} />
      </div>
      {!agent && (
        <div className="grid gap-2">
          <Label htmlFor="env-url">Docker URL</Label>
          <Input id="env-url" required className="font-mono" value={url} onChange={(e) => setUrl(e.target.value)} />
        </div>
      )}
    </FormDialog>
  );
}
