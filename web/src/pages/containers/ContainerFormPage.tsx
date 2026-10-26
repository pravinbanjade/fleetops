import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate, useParams, useSearch } from "@tanstack/react-router";
import { Plus, Trash2 } from "lucide-react";
import { useState, type FormEvent, type ReactNode } from "react";
import { toast } from "sonner";
import { containerSpecQuery, keys, networksQuery } from "@/api/queries";
import { containerAction, createContainer, recreateContainer } from "@/api/gen/sdk.gen";
import { DetailError } from "@/components/Details";
import { Page } from "@/components/Page";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { NativeSelect } from "@/components/ui/native-select";
import { Textarea } from "@/components/ui/textarea";
import { errorMessage } from "@/lib/errors";
import { EMPTY_FORM, forDuplicate, formToSpec, RESTART_POLICIES, specToForm, type SpecForm } from "@/lib/spec";
import { FormError } from "@/pages/AuthLayout";

type Mode = "create" | "duplicate" | "recreate";

/** `/containers/new` (optionally `?from=<id>` to duplicate). */
export function NewContainerPage() {
  const { env } = useParams({ from: "/authed/shell/envs/$env/containers/new" });
  const { from } = useSearch({ from: "/authed/shell/envs/$env/containers/new" });
  return from ? <Prefilled env={env} id={from} mode="duplicate" /> : <ContainerForm env={env} mode="create" initial={EMPTY_FORM} />;
}

/** `/containers/$id/edit`: recreate the container with changes. */
export function EditContainerPage() {
  const { env, id } = useParams({ from: "/authed/shell/envs/$env/containers/$id/edit" });
  return <Prefilled env={env} id={id} mode="recreate" />;
}

function Prefilled({ env, id, mode }: { env: string; id: string; mode: Mode }) {
  const q = useQuery(containerSpecQuery(env, id));
  if (q.error) {
    const back = (
      <Link to="/envs/$env/containers" params={{ env }} className="underline">
        Back to containers
      </Link>
    );
    return <DetailError error={q.error} what="container" back={back} />;
  }
  if (!q.data) return <p className="p-6 text-sm text-muted-foreground">Loading…</p>;
  if (mode === "recreate") {
    return <ContainerForm env={env} mode={mode} id={id} initial={specToForm(q.data)} original={q.data.name ?? id} />;
  }
  const initial = specToForm(forDuplicate(q.data));
  initial.name = initial.name ? `${initial.name}-copy` : "";
  const notice = q.data.ports?.some((p) => p.host_port != null)
    ? "Host ports are cleared so the copy doesn't clash with the original; set new ones if you need them."
    : undefined;
  return <ContainerForm env={env} mode={mode} initial={initial} original={q.data.name ?? id} notice={notice} />;
}

const TITLES: Record<Mode, (name?: string) => string> = {
  create: () => "Create container",
  duplicate: (n) => `Duplicate ${n}`,
  recreate: (n) => `Edit ${n}`,
};

function ContainerForm({
  env,
  mode,
  id,
  initial,
  original,
  notice,
}: {
  env: string;
  mode: Mode;
  id?: string;
  initial: SpecForm;
  original?: string;
  /** A note shown above the ports. */
  notice?: string;
}) {
  const qc = useQueryClient();
  const navigate = useNavigate();
  const networks = useQuery(networksQuery(env));
  const [f, setF] = useState(initial);
  const [start, setStart] = useState(true);
  const [invalid, setInvalid] = useState<string | null>(null);
  const set = <K extends keyof SpecForm>(k: K, v: SpecForm[K]) => setF((old) => ({ ...old, [k]: v }));

  const m = useMutation({
    meta: { inline: true },
    mutationFn: async (spec: ReturnType<typeof formToSpec>) => {
      if (mode === "recreate") {
        const { data } = await recreateContainer({ path: { env, id: id! }, body: spec });
        if (!data.old_removed) {
          toast.warning(`Recreated, but the old container could not be removed; it is kept as ${data.backup_name}`);
        } else {
          toast.success("Container recreated");
        }
        return data.id;
      }
      // Created and started in two steps, so a container that fails to
      // start is still opened rather than left behind unseen.
      const { data } = await createContainer({ path: { env }, body: { ...spec, start: false } });
      if (!start) {
        toast.success("Container created");
        return data.id;
      }
      try {
        await containerAction({ path: { env, id: data.id, action: "start" } });
        toast.success("Container created");
      } catch (e) {
        toast.error(`Created ${spec.name ?? data.id.slice(0, 12)}, but it could not start: ${errorMessage(e)}`);
      }
      return data.id;
    },
    onSuccess: async (newId) => {
      await qc.invalidateQueries({ queryKey: keys.containers(env) });
      void navigate({ to: "/envs/$env/containers/$id", params: { env, id: newId } });
    },
  });

  const submit = (e: FormEvent) => {
    e.preventDefault();
    try {
      const spec = formToSpec(f);
      setInvalid(null);
      m.mutate(spec);
    } catch (err) {
      setInvalid((err as Error).message);
    }
  };

  return (
    <Page title={TITLES[mode](original)}>
      {mode === "recreate" && (
        <p className="max-w-3xl text-sm text-muted-foreground">
          Saving stops this container, creates a replacement with these settings and starts it, then removes the old
          one. If the replacement fails to start, the old container is put back.
        </p>
      )}
      <form onSubmit={submit} className="grid max-w-3xl gap-6" aria-label="Container settings">
        <Section title="Basics">
          <Field id="image" label="Image">
            <Input id="image" required placeholder="nginx:1.27" value={f.image} onChange={(e) => set("image", e.target.value)} />
          </Field>
          <Field id="name" label="Name" hint="Leave empty for a generated name.">
            <Input id="name" value={f.name} onChange={(e) => set("name", e.target.value)} />
          </Field>
          <Field id="cmd" label="Command" hint="Leave empty for the image's default. Quote arguments with spaces.">
            <ArgsInput id="cmd" value={f.cmd} multiline={initial.cmd.includes("\n")} onChange={(v) => set("cmd", v)} />
          </Field>
          <Field id="entrypoint" label="Entrypoint">
            <ArgsInput
              id="entrypoint"
              value={f.entrypoint}
              multiline={initial.entrypoint.includes("\n")}
              onChange={(v) => set("entrypoint", v)}
            />
          </Field>
        </Section>

        <Section title="Ports">
          {notice && <p className="text-sm text-muted-foreground">{notice}</p>}
          <Ports rows={f.ports} onChange={(rows) => set("ports", rows)} />
        </Section>

        <Section title="Environment and storage">
          <Field id="env" label="Environment variables" hint="One KEY=value per line.">
            <Textarea id="env" className="font-mono" rows={4} value={f.env} onChange={(e) => set("env", e.target.value)} />
            <KeptNote names={f.kept.env.map((e) => e.split("=")[0])} />
          </Field>
          <Field id="binds" label="Volumes and bind mounts" hint="One source:target[:ro] per line, e.g. data:/var/lib/data.">
            <Textarea id="binds" className="font-mono" rows={3} value={f.binds} onChange={(e) => set("binds", e.target.value)} />
          </Field>
        </Section>

        <Section title="Network and restart">
          <Field id="network" label="Network">
            <NativeSelect id="network" value={f.network} onChange={(e) => set("network", e.target.value)}>
              <option value="">Default (bridge)</option>
              {networks.data
                ?.filter((n) => n.name !== "bridge")
                .map((n) => (
                  <option key={n.id} value={n.name}>
                    {n.name}
                  </option>
                ))}
              {f.network && !networks.data?.some((n) => n.name === f.network) && <option value={f.network}>{f.network}</option>}
            </NativeSelect>
          </Field>
          <Field id="restart" label="Restart policy">
            <NativeSelect
              id="restart"
              value={f.restart}
              onChange={(e) => set("restart", e.target.value as SpecForm["restart"])}
            >
              {RESTART_POLICIES.map((p) => (
                <option key={p} value={p}>
                  {p}
                </option>
              ))}
            </NativeSelect>
          </Field>
          <Field id="labels" label="Labels" hint="One key=value per line.">
            <Textarea id="labels" className="font-mono" rows={3} value={f.labels} onChange={(e) => set("labels", e.target.value)} />
            <KeptNote names={Object.keys(f.kept.labels)} />
          </Field>
        </Section>

        <Section title="Resources and privileges">
          <div className="grid gap-4 sm:grid-cols-2">
            <Field id="memory" label="Memory limit (MB)">
              <Input id="memory" inputMode="numeric" value={f.memoryMb} onChange={(e) => set("memoryMb", e.target.value)} />
            </Field>
            <Field id="cpus" label="CPUs" hint="For example 0.5 or 2.">
              <Input id="cpus" inputMode="decimal" value={f.cpus} onChange={(e) => set("cpus", e.target.value)} />
            </Field>
          </div>
          <Field id="caps" label="Added capabilities" hint="For example NET_ADMIN SYS_TIME.">
            <Input id="caps" className="font-mono" value={f.capAdd} onChange={(e) => set("capAdd", e.target.value)} />
          </Field>
          <div className="flex items-center gap-2">
            <Checkbox id="privileged" checked={f.privileged} onCheckedChange={(v) => set("privileged", !!v)} />
            <Label htmlFor="privileged">Privileged (full access to the host)</Label>
          </div>
        </Section>

        <Section title="Advanced">
          <Field
            id="advanced"
            label="Docker create JSON"
            hint="A ContainerCreate object merged over the settings above, e.g. {&quot;HostConfig&quot;: {&quot;ShmSize&quot;: 268435456}}."
          >
            <Textarea
              id="advanced"
              className="font-mono"
              rows={4}
              value={f.advanced}
              onChange={(e) => set("advanced", e.target.value)}
            />
          </Field>
        </Section>

        {mode !== "recreate" && (
          <div className="flex items-center gap-2">
            <Checkbox id="start" checked={start} onCheckedChange={(v) => setStart(!!v)} />
            <Label htmlFor="start">Start the container after creating it</Label>
          </div>
        )}
        <FormError message={invalid ?? (m.error ? errorMessage(m.error) : null)} />
        <div className="flex gap-2">
          <Button type="submit" disabled={m.isPending}>
            {mode === "recreate" ? "Recreate" : "Create"}
          </Button>
          <Button type="button" variant="outline" onClick={() => history.back()}>
            Cancel
          </Button>
        </div>
      </form>
    </Page>
  );
}

/** Command-line input; a textarea when the value spans lines (e.g. an `sh -c` script). */
function ArgsInput({
  id,
  value,
  multiline,
  onChange,
}: {
  id: string;
  value: string;
  multiline: boolean;
  onChange: (v: string) => void;
}) {
  return multiline ? (
    <Textarea id={id} className="font-mono" rows={4} value={value} onChange={(e) => onChange(e.target.value)} />
  ) : (
    <Input id={id} className="font-mono" value={value} onChange={(e) => onChange(e.target.value)} />
  );
}

/** Says which entries are carried unchanged because their values span lines. */
function KeptNote({ names }: { names: string[] }) {
  if (names.length === 0) return null;
  return (
    <p className="text-xs text-muted-foreground">
      {names.join(", ")} {names.length === 1 ? "has a multi-line value, which is" : "have multi-line values, which are"}{" "}
      kept unchanged.
    </p>
  );
}

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <fieldset className="grid gap-4 rounded-lg bg-[#1c1c1c] p-4">
      <legend className="float-left w-full border-b pb-3 text-sm font-semibold">{title}</legend>
      {children}
    </fieldset>
  );
}

function Field({ id, label, hint, children }: { id: string; label: string; hint?: string; children: ReactNode }) {
  return (
    <div className="grid gap-2">
      <Label htmlFor={id}>{label}</Label>
      {children}
      {hint && <p className="text-xs text-muted-foreground">{hint}</p>}
    </div>
  );
}

function Ports({ rows, onChange }: { rows: SpecForm["ports"]; onChange: (rows: SpecForm["ports"]) => void }) {
  const update = (i: number, patch: Partial<SpecForm["ports"][number]>) =>
    onChange(rows.map((r, j) => (j === i ? { ...r, ...patch } : r)));
  return (
    <div className="grid gap-2">
      {rows.map((r, i) => (
        <div key={i} className="flex items-center gap-2" role="group" aria-label={`Port ${i + 1}`}>
          <Input
            aria-label="Bind address"
            placeholder="all interfaces"
            className="w-36 shrink-0"
            value={r.hostIp}
            onChange={(e) => update(i, { hostIp: e.target.value })}
          />
          <Input
            aria-label="Host port"
            placeholder="host (random)"
            inputMode="numeric"
            value={r.host}
            onChange={(e) => update(i, { host: e.target.value })}
          />
          <span className="text-muted-foreground">→</span>
          <Input
            aria-label="Container port"
            placeholder="container"
            inputMode="numeric"
            value={r.container}
            onChange={(e) => update(i, { container: e.target.value })}
          />
          <NativeSelect
            aria-label="Protocol"
            className="w-24 shrink-0"
            value={r.protocol}
            onChange={(e) => update(i, { protocol: e.target.value as "tcp" | "udp" })}
          >
            <option value="tcp">tcp</option>
            <option value="udp">udp</option>
          </NativeSelect>
          <Button
            type="button"
            variant="ghost"
            size="icon"
            aria-label="Remove port"
            onClick={() => onChange(rows.filter((_, j) => j !== i))}
          >
            <Trash2 />
          </Button>
        </div>
      ))}
      <Button
        type="button"
        variant="outline"
        size="sm"
        className="justify-self-start"
        onClick={() => onChange([...rows, { host: "", hostIp: "", container: "", protocol: "tcp" }])}
      >
        <Plus />
        Add port
      </Button>
    </div>
  );
}
