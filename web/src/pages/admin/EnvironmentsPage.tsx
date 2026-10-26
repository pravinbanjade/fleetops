import { useQuery, useQueryClient } from "@tanstack/react-query";
import { CircleCheck, Container, KeyRound, Network, Pencil, ShieldCheck, ShipWheel, Trash2, type LucideIcon } from "lucide-react";
import { useId, useState } from "react";
import { Link } from "@tanstack/react-router";
import { toast } from "sonner";
import { envAccessQuery, envsQuery, envStatusQuery, keys } from "@/api/queries";
import { createEnv, deleteEnv, kubeContexts, rotateAgent, rotateSwarmKey, updateEnv } from "@/api/gen/sdk.gen";
import type { Enrollment, EnvKind, EnvView, KubeAccess, SwarmEnrollment as SwarmKey } from "@/api/gen/types.gen";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { columns, DataTable, type Column } from "@/components/DataTable";
import { Page } from "@/components/Page";
import type { QuickItem } from "@/components/RowMenu";
import { EnvTag } from "@/components/Shell";
import { StateBadge } from "@/components/StateBadge";
import { isEnterpriseBuild, Slot } from "@/ee-slots";
import { Button } from "@/components/ui/button";
import { Dialog as Modal, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { NativeSelect } from "@/components/ui/native-select";
import { Textarea } from "@/components/ui/textarea";
import { errorMessage } from "@/lib/errors";
import { formatAgo } from "@/lib/format";
import { AgentEnrollment } from "./AgentEnrollment";
import { SwarmEnrollment } from "./SwarmEnrollment";
import { FormDialog } from "./FormDialog";

type Dialog = { kind: "add" } | { kind: "edit" | "delete" | "rotate" | "access"; env: EnvView } | null;

const col = columns<EnvView>();

const isAgent = (e: EnvView) => e.kind === "docker-agent" || e.kind === "kube-agent" || e.kind === "swarm-agent";

/** An enrollment with the kind of environment it enrolls. */
type Pending = { enrollment: Enrollment; kind: EnvKind };

/** A Swarm environment's key and agent stack, after creating or rotating it. */
type SwarmPending = { env: string; enrollment: SwarmKey; rotated: boolean };

type Platform = "docker" | "kube" | "swarm";

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
  {
    id: "swarm",
    label: "Docker Swarm",
    hint: "A swarm, through an agent on every node",
    icon: Network,
    kinds: [["swarm-agent", "Agent on every node"]],
  },
];

/** How the list names each kind: the platform, and how FleetOps reaches it when that isn't the default. */
const KIND_LABEL: Record<EnvKind, string> = {
  "docker-local": "Docker",
  "docker-agent": "Docker via agent",
  "kube-config": "Kubernetes",
  "kube-agent": "Kubernetes via agent",
  "swarm-agent": "Docker Swarm",
  "swarm-node": "Swarm node",
};

const platformOf = (k: EnvKind): Platform => (k.startsWith("kube") ? "kube" : k.startsWith("swarm") ? "swarm" : "docker");

const DESCRIPTIONS: Record<EnvKind, string> = {
  "docker-local": "A Docker engine this server can reach: a unix socket, or tcp:// / http(s):// for an engine exposing its API.",
  "docker-agent": "A remote host running fleetops-agent, which connects out to this server. You get the command to run there next.",
  "kube-config":
    "Paste a kubeconfig with a ServiceAccount token or client certificate. It is stored encrypted and never shown again. exec plugins (aws, gcloud) are not supported: use the in-cluster agent instead.",
  "kube-agent": "You'll get a kubectl command that installs the agent in the cluster. The cluster needs no inbound access.",
  "swarm-agent":
    "A Docker Swarm. You'll get a key and a stack file that runs the agent on every node; each node's agent connects out to this server.",
  "swarm-node": "One node of a Docker Swarm, reached through its swarm.",
};

/** What an in-cluster agent may do, from least to most. */
const ACCESS: [KubeAccess, string, string][] = [
  ["read-only", "Read-only", "Browse and read logs. No changes, no terminal."],
  ["standard", "Standard", "Change the common kinds: create, edit, scale, restart, delete, terminal."],
  ["cluster-admin", "Cluster admin", "Apply any kind, including CRDs and RBAC: full control of the cluster."],
];

/** Up or down as a holder, with why it's down beside it. */
function EnvState({ env }: { env: EnvView }) {
  const { data } = useQuery(envStatusQuery(env.name));
  if (!data) return null;
  if (data.state === "up") return <StateBadge state="up" />;
  return (
    <span className="flex min-w-0 items-center gap-2">
      <StateBadge state="down" />
      {data.reason && (
        <span className="truncate text-xs text-muted-foreground" title={data.reason}>
          {data.reason}
        </span>
      )}
    </span>
  );
}

/** What an agent environment's agent is doing, for the list. */
function AgentCell({ env }: { env: EnvView }) {
  const a = env.agent;
  if (!isAgent(env) || !a) return null;
  if (env.kind === "swarm-agent") {
    const n = env.swarm_nodes_connected ?? 0;
    return (
      <span className="grid gap-0.5">
        <span className={n ? undefined : "text-muted-foreground"}>
          {n ? `${n} node${n === 1 ? "" : "s"} connected${a.connected ? "" : ", no manager"}` : "waiting for agents"}
        </span>
        {a.last_error && <span className="text-xs text-destructive">{a.last_error}</span>}
      </span>
    );
  }
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
  const [swarmKey, setSwarmKey] = useState<SwarmPending | null>(null);
  const close = () => setDialog(null);
  // Renaming or removing one changes every environment-scoped key, so all are refetched.
  const refresh = () => Promise.all([qc.invalidateQueries({ queryKey: keys.envs }), qc.invalidateQueries({ queryKey: ["env"] })]);
  const [cols] = useState<Column<EnvView>[]>(() => [
    col.accessor("name", {
      header: "Name",
      cell: (c) => (
        <Link to="/envs/$env" params={{ env: c.getValue() }} className="flex items-center gap-2 font-medium underline-offset-2 hover:underline">
          <EnvTag name={c.getValue()} size="sm" />
          {c.getValue()}
        </Link>
      ),
    }),
    col.display({ id: "status", header: "Status", cell: (c) => <EnvState env={c.row.original} /> }),
    col.display({ id: "agent", header: "Agent", cell: (c) => <AgentCell env={c.row.original} /> }),
    col.accessor((e) => KIND_LABEL[e.kind as EnvKind] ?? e.kind, { id: "kind", header: "Kind" }),
    col.accessor((e) => (e.kube ? `${e.kube.server} · ${e.kube.context} (${e.kube.user_kind === "exec" ? `via ${e.kube.exec_command}` : e.kube.user_kind})` : e.url), {
      id: "url",
      header: "URL",
      cell: (c) => (
        <code className="block max-w-[16rem] truncate text-xs" title={c.getValue()}>
          {c.getValue()}
        </code>
      ),
    }),
    col.display({
      id: "access",
      header: () => <span className="sr-only">Access</span>,
      cell: (c) => (
        <Button variant="outline" size="sm" onClick={() => setDialog({ kind: "access", env: c.row.original })}>
          <ShieldCheck aria-hidden />
          Access
        </Button>
      ),
    }),
  ]);
  const menu = (e: EnvView): QuickItem[] => [
    { id: "edit", label: "Edit", icon: Pencil, onSelect: () => setDialog({ kind: "edit", env: e }) },
    ...(isAgent(e) ? [{ id: "rotate", label: "Rotate key", icon: KeyRound, onSelect: () => setDialog({ kind: "rotate", env: e }) }] : []),
    { id: "delete", label: "Delete", icon: Trash2, danger: true, onSelect: () => setDialog({ kind: "delete", env: e }) },
  ];
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
        phoneColumns={["name", "status", "access"]}
        rowMenu={{ label: (e) => e.name, items: menu }}
      />
      {dialog?.kind === "add" && (
        <EnvForm onClose={close} onDone={refresh} onEnrollment={setEnrollment} onSwarmKey={(env, e) => setSwarmKey({ env, enrollment: e, rotated: false })} />
      )}
      {dialog?.kind === "edit" && <EnvForm env={dialog.env} onClose={close} onDone={refresh} />}
      {dialog?.kind === "rotate" && dialog.env.kind === "swarm-agent" && (
        <ConfirmDialog
          open
          onOpenChange={(open) => !open && close()}
          title={`Rotate key for ${dialog.env.name}?`}
          description="You get a new key and stack file. The current key keeps working for 24 hours, or until you finish the rotation, so running agents stay connected while you update the secret."
          confirmLabel="Rotate"
          onConfirm={async () => {
            const { data } = await rotateSwarmKey({ path: { env: dialog.env.name }, throwOnError: true });
            setSwarmKey({ env: dialog.env.name, enrollment: data, rotated: true });
            await refresh();
          }}
        />
      )}
      {dialog?.kind === "rotate" && dialog.env.kind !== "swarm-agent" && (
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
      {swarmKey && (
        <SwarmEnrollment env={swarmKey.env} enrollment={swarmKey.enrollment} rotated={swarmKey.rotated} onClose={() => setSwarmKey(null)} />
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
  onSwarmKey,
}: {
  env?: EnvView;
  onClose: () => void;
  onDone: () => Promise<unknown>;
  onEnrollment?: (p: Pending) => void;
  onSwarmKey?: (env: string, e: SwarmKey) => void;
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
  const [customResources, setCustomResources] = useState(false);
  const agent = kind === "docker-agent" || kind === "kube-agent" || kind === "swarm-agent";

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
            body: { name: name.trim(), kind, ...(kind === "kube-agent" ? { access, custom_resources: customResources } : {}) },
            throwOnError: true,
          });
          if (data.enrollment) onEnrollment?.({ enrollment: data.enrollment, kind });
          if (data.swarm_enrollment) onSwarmKey?.(data.name, data.swarm_enrollment);
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
      {!env && kind === "kube-agent" && (
        <label className="flex cursor-pointer items-start gap-2 text-sm">
          <Checkbox className="mt-0.5" checked={customResources} onCheckedChange={(v) => setCustomResources(v === true)} />
          <span className="grid">
            <span className="font-medium">Read custom resources</span>
            <span className="text-xs text-muted-foreground">
              Lets FleetOps list every kind, including CRDs. This also reads every built-in kind, Secrets included.
            </span>
          </span>
        </label>
      )}
      {kind === "kube-config" && (
        <>
          <div className="grid gap-2">
            <Label htmlFor="env-kubeconfig">{env ? "Replace kubeconfig" : "Kubeconfig"}</Label>
            <Textarea
              id="env-kubeconfig"
              required={!env}
              rows={8}
              spellCheck={false}
              className="font-mono text-xs"
              value={kubeconfig}
              onChange={(e) => setKubeconfig(e.target.value)}
              onBlur={(e) => void loadContexts(e.target.value)}
              onPaste={(e) => {
                const el = e.currentTarget;
                setTimeout(() => void loadContexts(el.value), 0);
              }}
            />
            {contextsError && <p className="text-sm text-state-fail-text">{contextsError}</p>}
          </div>
          {contexts.length > 0 && (
            <div className="grid gap-2">
              <Label htmlFor="env-context">Context</Label>
              <NativeSelect id="env-context" value={context} onChange={(e) => setContext(e.target.value)}>
                {contexts.map((c) => (
                  <option key={c} value={c}>
                    {c}
                  </option>
                ))}
              </NativeSelect>
            </div>
          )}
          <label className="flex items-center gap-2 text-sm">
            <Checkbox checked={skipCheck} onCheckedChange={(v) => setSkipCheck(v === true)} />
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
      <fieldset className="grid gap-3 sm:grid-cols-3">
        <legend className="sr-only">Platform</legend>
        {PLATFORMS.map((p) => {
          const on = p.id === platform.id;
          const Icon = p.icon;
          return (
            <label
              key={p.id}
              className={
                "relative grid cursor-pointer gap-3 rounded-lg border p-3 transition-colors has-[:focus-visible]:ring-2 has-[:focus-visible]:ring-ring " +
                (on ? "border-primary bg-selected" : "bg-background/40 hover:bg-accent")
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
        <div
          className="grid gap-1 rounded-md border bg-background/40 p-1"
          style={{ gridTemplateColumns: `repeat(${platform.kinds.length}, minmax(0, 1fr))` }}
        >
          {platform.kinds.map(([k, label]) => (
            <label
              key={k}
              className={
                "flex h-8 cursor-pointer items-center justify-center rounded-sm px-2 text-sm press has-[:focus-visible]:ring-2 has-[:focus-visible]:ring-ring " +
                (kind === k ? "bg-selected font-medium text-foreground shadow-[inset_0_0_0_1px_var(--primary)]" : "text-muted-foreground hover:bg-accent hover:text-foreground")
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
          <p className="text-sm text-muted-foreground">
            {isEnterpriseBuild ? "No user has a direct grant here." : "Only admins can use this environment."}
          </p>
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
                  <td className="py-1.5">{a.role_name ?? a.role}</td>
                  <td className="py-1.5 font-mono text-xs">{a.namespaces ? a.namespaces.join(", ") : "Entire environment"}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
        <Slot name="env.access" props={{ env }} />
      </DialogContent>
    </Modal>
  );
}
