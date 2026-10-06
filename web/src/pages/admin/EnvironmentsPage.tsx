import { useQuery, useQueryClient } from "@tanstack/react-query";
import { CircleCheck, Container, ShipWheel, type LucideIcon } from "lucide-react";
import { useId, useState } from "react";
import { Link } from "@tanstack/react-router";
import { toast } from "sonner";
import { envAccessQuery, envsQuery, keys } from "@/api/queries";
import { createEnv, deleteEnv, kubeContexts, rotateAgent, updateEnv } from "@/api/gen/sdk.gen";
import type { Enrollment, EnvKind, EnvView, KubeAccess } from "@/api/gen/types.gen";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { columns, DataTable, type Column } from "@/components/DataTable";
import { Page } from "@/components/Page";
import { EnvStatusBadge } from "@/components/Shell";
import { Button } from "@/components/ui/button";
import { Dialog as Modal, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { errorMessage } from "@/lib/errors";
import { formatAgo } from "@/lib/format";
import { AgentEnrollment } from "./AgentEnrollment";
import { FormDialog } from "./FormDialog";

type Dialog = { kind: "add" } | { kind: "edit" | "delete" | "rotate" | "access"; env: EnvView } | null;

const col = columns<EnvView>();

const isAgent = (e: EnvView) => e.kind === "docker-agent" || e.kind === "kube-agent";

/** An enrollment with the kind of environment it enrolls. */
type Pending = { enrollment: Enrollment; kind: EnvKind };

type Platform = "docker" | "kube";

/** The two things an environment can be, each with the ways FleetOps reaches it (the first is the default). */
const PLATFORMS: { id: Platform; label: string; hint: string; icon: LucideIcon; kinds: [EnvKind, string][] }[] = [
  {
    id: "docker",
    label: "Docker",
    hint: "An engine on this server or a remote host",
    icon: Container,
    kinds: [
      ["docker-local", "Socket or API URL"],
      ["docker-agent", "Agent on a remote host"],
    ],
  },
  {
    id: "kube",
    label: "Kubernetes",
    hint: "A cluster, through its API or an agent inside it",
    icon: ShipWheel,
    kinds: [
      ["kube-config", "Paste a kubeconfig"],
      ["kube-agent", "In-cluster agent"],
    ],
  },
];

const platformOf = (k: EnvKind): Platform => (k.startsWith("kube") ? "kube" : "docker");

const DESCRIPTIONS: Record<EnvKind, string> = {
  "docker-local": "A Docker engine this server can reach: a unix socket, or tcp:// / http(s):// for an engine exposing its API.",
  "docker-agent": "A remote host running fleetops-agent, which connects out to this server. You get the command to run there next.",
  "kube-config":
    "Paste a kubeconfig with a ServiceAccount token or client certificate. It is stored encrypted and never shown again. exec plugins (aws, gcloud) are not supported: use the in-cluster agent instead.",
  "kube-agent": "You'll get a kubectl command that installs the agent in the cluster. The cluster needs no inbound access.",
};

/** What an in-cluster agent may do, from least to most. */
const ACCESS: [KubeAccess, string, string][] = [
  ["read-only", "Read-only", "Browse and read logs. No changes, no terminal."],
  ["standard", "Standard", "Change the common kinds: create, edit, scale, restart, delete, terminal."],
  ["cluster-admin", "Cluster admin", "Apply any kind, including CRDs and RBAC: full control of the cluster."],
];

/** What an agent environment's agent is doing, for the list. */
function AgentCell({ env }: { env: EnvView }) {
  const a = env.agent;
  if (!isAgent(env) || !a) return null;
  if (!a.enrolled) return <span className="text-muted-foreground">waiting for agent</span>;
  return (
    <span className="grid gap-0.5">
      <span className="font-mono text-xs">{a.agent_version}</span>
      {a.access && <span className="text-xs text-muted-foreground">{a.access}</span>}
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
  const [enrollment, setEnrollment] = useState<Pending | null>(null);
  const close = () => setDialog(null);
  // Renaming or removing one changes every environment-scoped key, so all are refetched.
  const refresh = () => Promise.all([qc.invalidateQueries({ queryKey: keys.envs }), qc.invalidateQueries({ queryKey: ["env"] })]);
  const [cols] = useState<Column<EnvView>[]>(() => [
    col.accessor("name", { header: "Name", cell: (c) => <span className="font-medium">{c.getValue()}</span> }),
    col.display({ id: "status", header: "Status", cell: (c) => <EnvStatusBadge env={c.row.original.name} /> }),
    col.display({ id: "agent", header: "Agent", cell: (c) => <AgentCell env={c.row.original} /> }),
    col.accessor("kind", { header: "Kind" }),
    col.accessor((e) => (e.kube ? `${e.kube.server} · ${e.kube.context} (${e.kube.user_kind})` : e.url), {
      id: "url",
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
          <Button variant="outline" size="sm" onClick={() => setDialog({ kind: "access", env: c.row.original })}>
            Access
          </Button>
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
            setEnrollment({ enrollment: data, kind: dialog.env.kind as EnvKind });
            await refresh();
          }}
        />
      )}
      {enrollment && (
        <AgentEnrollment enrollment={enrollment.enrollment} kind={enrollment.kind} onClose={() => setEnrollment(null)} />
      )}
      {dialog?.kind === "access" && <EnvAccess env={dialog.env.name} onClose={close} />}
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
  onEnrollment?: (p: Pending) => void;
}) {
  const [name, setName] = useState(env?.name ?? "");
  const [url, setUrl] = useState(env?.url ?? "unix:///var/run/docker.sock");
  const [kind, setKind] = useState<EnvKind>((env?.kind as EnvKind | undefined) ?? "docker-local");
  const [kubeconfig, setKubeconfig] = useState("");
  const [contexts, setContexts] = useState<string[]>([]);
  const [context, setContext] = useState<string | undefined>();
  const [contextsError, setContextsError] = useState<string | null>(null);
  const [skipCheck, setSkipCheck] = useState(false);
  const [access, setAccess] = useState<KubeAccess>("standard");
  const agent = kind === "docker-agent" || kind === "kube-agent";

  const loadContexts = async (text: string) => {
    setContextsError(null);
    if (!text.trim()) return setContexts([]);
    try {
      const { data } = await kubeContexts({ body: { kubeconfig: text }, throwOnError: true });
      setContexts(data.contexts);
      setContext(data.current ?? data.contexts[0]);
    } catch (e) {
      setContexts([]);
      setContextsError(errorMessage(e));
    }
  };

  return (
    <FormDialog
      title={env ? `Edit ${env.name}` : "Add environment"}
      description={env ? DESCRIPTIONS[kind] : "Pick what FleetOps manages and how it reaches it."}
      submitLabel={env ? "Save" : "Add"}
      onClose={onClose}
      onSubmit={async () => {
        if (env) {
          const body = {
            name: name.trim() === env.name ? null : name.trim(),
            url: kind !== "docker-local" || url.trim() === env.url ? null : url.trim(),
            ...(kind === "kube-config" && kubeconfig.trim()
              ? { kubeconfig, skip_check: skipCheck, ...(context ? { context } : {}) }
              : {}),
          };
          if (!body.name && !body.url && !("kubeconfig" in body)) return;
          await updateEnv({ path: { env: env.name }, body, throwOnError: true });
        } else if (kind === "kube-config") {
          await createEnv({
            body: { name: name.trim(), kind, kubeconfig, ...(context ? { context } : {}), skip_check: skipCheck },
            throwOnError: true,
          });
        } else if (agent) {
          const { data } = await createEnv({
            body: { name: name.trim(), kind, ...(kind === "kube-agent" ? { access } : {}) },
            throwOnError: true,
          });
          if (data.enrollment) onEnrollment?.({ enrollment: data.enrollment, kind });
        } else {
          await createEnv({ body: { name: name.trim(), url: url.trim(), kind } });
        }
        await onDone();
      }}
    >
      {!env && <KindPicker kind={kind} onChange={setKind} />}
      <div className="grid gap-2">
        <Label htmlFor="env-name">Name</Label>
        <Input id="env-name" required value={name} onChange={(e) => setName(e.target.value)} />
      </div>
      {kind === "docker-local" && (
        <div className="grid gap-2">
          <Label htmlFor="env-url">Docker URL</Label>
          <Input id="env-url" required className="font-mono" value={url} onChange={(e) => setUrl(e.target.value)} />
        </div>
      )}
      {!env && kind === "kube-agent" && (
        <fieldset className="grid gap-2">
          <legend className="mb-1 text-sm font-medium">Access in the cluster</legend>
          {ACCESS.map(([value, label, hint]) => (
            <label key={value} className="flex cursor-pointer items-start gap-2 rounded-md border p-2 text-sm has-[:checked]:border-primary">
              <input type="radio" name="env-access" className="mt-1" checked={access === value} onChange={() => setAccess(value)} />
              <span className="grid">
                <span className="font-medium">{label}</span>
                <span className="text-xs text-muted-foreground">{hint}</span>
              </span>
            </label>
          ))}
        </fieldset>
      )}
      {kind === "kube-config" && (
        <>
          <div className="grid gap-2">
            <Label htmlFor="env-kubeconfig">{env ? "Replace kubeconfig" : "Kubeconfig"}</Label>
            <textarea
              id="env-kubeconfig"
              required={!env}
              rows={8}
              spellCheck={false}
              className="rounded-md border bg-background p-2 font-mono text-xs"
              value={kubeconfig}
              onChange={(e) => setKubeconfig(e.target.value)}
              onBlur={(e) => void loadContexts(e.target.value)}
              onPaste={(e) => {
                const el = e.currentTarget;
                setTimeout(() => void loadContexts(el.value), 0);
              }}
            />
            {contextsError && <p className="text-sm text-destructive">{contextsError}</p>}
          </div>
          {contexts.length > 0 && (
            <div className="grid gap-2">
              <Label htmlFor="env-context">Context</Label>
              <select
                id="env-context"
                aria-label="Context"
                className="h-9 rounded-md border bg-background px-2"
                value={context}
                onChange={(e) => setContext(e.target.value)}
              >
                {contexts.map((c) => (
                  <option key={c} value={c}>
                    {c}
                  </option>
                ))}
              </select>
            </div>
          )}
          <label className="flex items-center gap-2 text-sm">
            <input type="checkbox" checked={skipCheck} onChange={(e) => setSkipCheck(e.target.checked)} />
            Save without checking that the cluster answers
          </label>
        </>
      )}
    </FormDialog>
  );
}

/** Platform cards, then the connection method within the chosen platform, then what that method means. */
function KindPicker({ kind, onChange }: { kind: EnvKind; onChange: (k: EnvKind) => void }) {
  const id = useId();
  const platform = PLATFORMS.find((p) => p.id === platformOf(kind))!;
  return (
    <div className="grid gap-3">
      <fieldset className="grid grid-cols-2 gap-3">
        <legend className="sr-only">Platform</legend>
        {PLATFORMS.map((p) => {
          const on = p.id === platform.id;
          const Icon = p.icon;
          return (
            <label
              key={p.id}
              className={
                "relative grid cursor-pointer gap-3 rounded-lg border p-3 transition-colors has-[:focus-visible]:ring-2 has-[:focus-visible]:ring-ring " +
                (on ? "border-primary bg-cord-soft" : "bg-background/40 hover:bg-accent")
              }
            >
              <input
                type="radio"
                name="env-platform"
                className="sr-only"
                checked={on}
                onChange={() => onChange(p.kinds[0][0])}
                aria-labelledby={`${id}-${p.id}`}
                aria-describedby={`${id}-${p.id}-hint`}
              />
              <span
                className={
                  "grid size-9 place-items-center rounded-md border " + (on ? "border-primary/40 text-primary" : "text-muted-foreground")
                }
              >
                <Icon className="size-5" aria-hidden />
              </span>
              <span className="grid gap-0.5">
                <span id={`${id}-${p.id}`} className="text-sm font-semibold">
                  {p.label}
                </span>
                <span id={`${id}-${p.id}-hint`} className="text-xs text-muted-foreground">
                  {p.hint}
                </span>
              </span>
              {on && <CircleCheck className="absolute top-3 right-3 size-4 text-primary" aria-hidden />}
            </label>
          );
        })}
      </fieldset>
      <fieldset className="grid gap-2">
        <legend className="mb-2 text-sm font-medium">Connect with</legend>
        <div className="grid grid-cols-2 gap-1 rounded-md border bg-background/40 p-1">
          {platform.kinds.map(([k, label]) => (
            <label
              key={k}
              className={
                "flex h-8 cursor-pointer items-center justify-center rounded-sm px-2 text-sm transition-colors has-[:focus-visible]:ring-2 has-[:focus-visible]:ring-ring " +
                (kind === k ? "bg-cord-soft font-medium text-foreground shadow-[inset_0_0_0_1px_var(--primary)]" : "text-muted-foreground hover:bg-accent hover:text-foreground")
              }
            >
              <input type="radio" name="env-kind" className="sr-only" checked={kind === k} onChange={() => onChange(k)} />
              {label}
            </label>
          ))}
        </div>
        <p className="text-xs text-muted-foreground">{DESCRIPTIONS[kind]}</p>
      </fieldset>
    </div>
  );
}

/** Who may use `env`, read-only; grants are changed per user. */
function EnvAccess({ env, onClose }: { env: string; onClose: () => void }) {
  const q = useQuery(envAccessQuery(env));
  return (
    <Modal open onOpenChange={(open) => !open && onClose()}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Access to {env}</DialogTitle>
          <DialogDescription>
            Admins have full access everywhere. Change a user&apos;s access from{" "}
            <Link to="/admin/users" className="underline" onClick={onClose}>
              Users
            </Link>
            .
          </DialogDescription>
        </DialogHeader>
        {q.error ? (
          <p role="alert" className="text-sm text-destructive">
            {errorMessage(q.error)}
          </p>
        ) : !q.data ? (
          <p className="text-sm text-muted-foreground">Loading…</p>
        ) : q.data.length === 0 ? (
          <p className="text-sm text-muted-foreground">Only admins can use this environment.</p>
        ) : (
          <table aria-label={`Access to ${env}`} className="w-full text-sm">
            <thead className="text-left text-muted-foreground">
              <tr>
                <th className="py-1 font-normal">User</th>
                <th className="py-1 font-normal">Role</th>
                <th className="py-1 font-normal">Namespaces</th>
              </tr>
            </thead>
            <tbody>
              {q.data.map((a) => (
                <tr key={a.user_id} className="border-t">
                  <td className="py-1.5 font-medium">{a.username}</td>
                  <td className="py-1.5">{a.role}</td>
                  <td className="py-1.5 font-mono text-xs">{a.namespaces ? a.namespaces.join(", ") : "Entire environment"}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </DialogContent>
    </Modal>
  );
}
