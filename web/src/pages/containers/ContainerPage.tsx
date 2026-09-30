import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate, useParams, useSearch } from "@tanstack/react-router";
import { lazy, Suspense, useState, type FormEvent } from "react";
import { containerQuery, keys } from "@/api/queries";
import { containerAction, renameContainer } from "@/api/gen/sdk.gen";
import type { ContainerActionName } from "@/api/gen/types.gen";
import type { ContainerInspect } from "@/api/docker";
import { Details, DetailError } from "@/components/Details";
import { JsonView } from "@/components/JsonView";
import { Page } from "@/components/Page";
import { StateBadge } from "@/components/StateBadge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { joinArgs } from "@/lib/args";
import { errorMessage } from "@/lib/errors";
import { shortId } from "@/lib/format";
import { isSecretKey, maskEnv, splitEnv } from "@/lib/secrets";
import { FormError } from "@/pages/AuthLayout";
import { ACTIONS, actionsFor, RemoveDialog } from "./actions";
import { FilesTab } from "./FilesTab";
import { LogsTab } from "./logs/LogsTab";
import type { ContainerTab } from "./tabs";

const FROM = "/authed/shell/envs/$env/containers/$id";

// xterm.js is large; it loads only when the Terminal tab opens.
const TerminalTab = lazy(() => import("./terminal/TerminalTab").then((m) => ({ default: m.TerminalTab })));

export function ContainerPage() {
  const { env, id } = useParams({ from: FROM });
  const { tab = "overview", path = "/" } = useSearch({ from: FROM });
  const navigate = useNavigate({ from: "/envs/$env/containers/$id" });
  const q = useQuery(containerQuery(env, id));
  const back = (
    <Link to="/envs/$env/containers" params={{ env }} className="underline">
      Back to containers
    </Link>
  );
  if (q.error) return <DetailError error={q.error} what="container" back={back} />;
  const c = q.data;
  if (!c) return <p className="p-6 text-sm text-muted-foreground">Loading…</p>;
  const name = c.Name.replace(/^\//, "");
  return (
    <Page
      title={
        <span className="flex items-center gap-3">
          {name} <StateBadge state={c.State.Health?.Status ?? c.State.Status} />
        </span>
      }
      actions={<ContainerActions env={env} c={c} name={name} />}
    >
      <Tabs value={tab} onValueChange={(t) => void navigate({ search: { tab: t as ContainerTab }, replace: true })}>
        <TabsList>
          <TabsTrigger value="overview">Overview</TabsTrigger>
          <TabsTrigger value="inspect">Inspect</TabsTrigger>
          <TabsTrigger value="env">Env</TabsTrigger>
          <TabsTrigger value="mounts">Mounts</TabsTrigger>
          <TabsTrigger value="networks">Networks</TabsTrigger>
          <TabsTrigger value="logs">Logs</TabsTrigger>
          <TabsTrigger value="terminal">Terminal</TabsTrigger>
          <TabsTrigger value="files">Files</TabsTrigger>
        </TabsList>
        <TabsContent value="overview">
          <Overview c={c} />
        </TabsContent>
        <TabsContent value="inspect">
          <JsonView value={c} masked={{ ...c, Config: { ...c.Config, Env: maskEnv(c.Config.Env ?? []) } }} label="Container JSON" />
        </TabsContent>
        <TabsContent value="env">
          <EnvTable env={c.Config.Env ?? []} />
        </TabsContent>
        <TabsContent value="mounts">
          <Mounts c={c} />
        </TabsContent>
        <TabsContent value="networks">
          <Networks c={c} />
        </TabsContent>
        <TabsContent value="logs">
          <LogsTab env={env} id={id} name={name} running={c.State.Running === true} />
        </TabsContent>
        <TabsContent value="terminal">
          <Suspense fallback={<p className="text-sm text-muted-foreground">Loading…</p>}>
            <TerminalTab env={env} id={id} running={c.State.Running === true} />
          </Suspense>
        </TabsContent>
        <TabsContent value="files">
          <FilesTab env={env} id={id} path={path} />
        </TabsContent>
      </Tabs>
    </Page>
  );
}

function ContainerActions({ env, c, name }: { env: string; c: ContainerInspect; name: string }) {
  const qc = useQueryClient();
  const navigate = useNavigate();
  const [renaming, setRenaming] = useState(false);
  const [removing, setRemoving] = useState(false);
  const act = useMutation({
    mutationFn: (action: ContainerActionName) => containerAction({ path: { env, id: c.Id, action } }),
    onSettled: () => qc.invalidateQueries({ queryKey: keys.containers(env) }),
  });
  return (
    <>
      {actionsFor(c.State.Status).map((a) => (
        <Button key={a} variant="outline" disabled={act.isPending} onClick={() => act.mutate(a)}>
          {ACTIONS.find((x) => x.action === a)!.label}
        </Button>
      ))}
      <Button variant="outline" onClick={() => setRenaming(true)}>
        Rename
      </Button>
      <Button variant="outline" asChild>
        <Link to="/envs/$env/containers/$id/edit" params={{ env, id: c.Id }}>
          Edit
        </Link>
      </Button>
      <Button variant="outline" asChild>
        <Link to="/envs/$env/containers/new" params={{ env }} search={{ from: c.Id }}>
          Duplicate
        </Link>
      </Button>
      <Button variant="destructive" onClick={() => setRemoving(true)}>
        Remove
      </Button>
      {renaming && <RenameDialog env={env} id={c.Id} name={name} onClose={() => setRenaming(false)} />}
      {removing && (
        <RemoveDialog
          env={env}
          containers={[{ id: c.Id, name }]}
          onDone={(removed) => {
            setRemoving(false);
            if (removed) void navigate({ to: "/envs/$env/containers", params: { env } });
          }}
        />
      )}
    </>
  );
}

function RenameDialog({ env, id, name, onClose }: { env: string; id: string; name: string; onClose: () => void }) {
  const qc = useQueryClient();
  const [value, setValue] = useState(name);
  const m = useMutation({
    mutationFn: () => renameContainer({ path: { env, id }, body: { name: value } }),
    meta: { inline: true },
    onSuccess: async () => {
      await qc.invalidateQueries({ queryKey: keys.containers(env) });
      onClose();
    },
  });
  const submit = (e: FormEvent) => {
    e.preventDefault();
    m.mutate();
  };
  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Rename {name}</DialogTitle>
        </DialogHeader>
        <form onSubmit={submit} className="grid gap-4">
          <div className="grid gap-2">
            <Label htmlFor="rename">New name</Label>
            <Input id="rename" required value={value} onChange={(e) => setValue(e.target.value)} />
          </div>
          <FormError message={m.error ? errorMessage(m.error) : null} />
          <DialogFooter>
            <Button type="submit" disabled={m.isPending}>
              Rename
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

/** A bind address as it is written before `:port`: IPv6 in brackets. */
const hostAddr = (ip: string) => (!ip ? "0.0.0.0" : ip.includes(":") ? `[${ip}]` : ip);

function Overview({ c }: { c: ContainerInspect }) {
  const restart = c.HostConfig.RestartPolicy;
  const ports = Object.entries(c.NetworkSettings.Ports ?? {}).flatMap(([port, binds]) =>
    binds?.length ? binds.map((b) => `${hostAddr(b.HostIp)}:${b.HostPort} → ${port}`) : [port],
  );
  const labels = Object.entries(c.Config.Labels ?? {});
  return (
    <div className="grid gap-6">
      <Details
        label="Container details"
        items={[
          ["ID", <code key="id">{shortId(c.Id)}</code>],
          ["Image", c.Config.Image],
          ["State", c.State.Status + (c.State.Status === "exited" ? ` (exit code ${c.State.ExitCode})` : "")],
          ["Error", c.State.Error],
          ["Started", c.State.Running ? new Date(c.State.StartedAt!).toLocaleString() : undefined],
          ["Created", new Date(c.Created).toLocaleString()],
          ["Command", joinArgs([...(c.Config.Entrypoint ?? []), ...(c.Config.Cmd ?? [])])],
          ["Restart policy", restart && restart.Name !== "no" && restart.Name ? restart.Name : "no"],
          ["Ports", ports.length ? ports.join(", ") : "none"],
        ]}
      />
      <section className="grid gap-2">
        <h2 className="font-medium">Labels</h2>
        {labels.length === 0 ? (
          <p className="text-sm text-muted-foreground">No labels.</p>
        ) : (
          <Details label="Labels" items={labels.map(([k, v]) => [k, v])} />
        )}
      </section>
    </div>
  );
}

/** The container's environment, with secret-looking values hidden until clicked. */
export function EnvTable({ env }: { env: string[] }) {
  const [shown, setShown] = useState<Set<string>>(new Set());
  if (env.length === 0) return <p className="text-sm text-muted-foreground">No environment variables.</p>;
  return (
    <Table aria-label="Environment">
      <TableHeader>
        <TableRow>
          <TableHead>Name</TableHead>
          <TableHead>Value</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {env.map(splitEnv).map(([k, v]) => (
          <TableRow key={k}>
            <TableCell className="font-mono text-xs">{k}</TableCell>
            <TableCell className="font-mono text-xs whitespace-normal break-all">
              {isSecretKey(k) && !shown.has(k) ? (
                <button
                  type="button"
                  className="text-muted-foreground hover:underline"
                  aria-label={`Show ${k}`}
                  onClick={() => setShown(new Set(shown).add(k))}
                >
                  ••••••••
                </button>
              ) : (
                v
              )}
            </TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}

function Mounts({ c }: { c: ContainerInspect }) {
  const mounts = c.Mounts ?? [];
  if (mounts.length === 0) return <p className="text-sm text-muted-foreground">No mounts.</p>;
  return (
    <Table aria-label="Mounts">
      <TableHeader>
        <TableRow>
          <TableHead>Type</TableHead>
          <TableHead>Source</TableHead>
          <TableHead>Destination</TableHead>
          <TableHead>Mode</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {mounts.map((m) => (
          <TableRow key={m.Destination}>
            <TableCell>{m.Type}</TableCell>
            <TableCell className="font-mono text-xs">{m.Type === "volume" && m.Name ? m.Name : m.Source}</TableCell>
            <TableCell className="font-mono text-xs">{m.Destination}</TableCell>
            <TableCell>{m.RW ? "read-write" : "read-only"}</TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}

function Networks({ c }: { c: ContainerInspect }) {
  const nets = Object.entries(c.NetworkSettings.Networks ?? {});
  if (nets.length === 0) return <p className="text-sm text-muted-foreground">Not attached to any network.</p>;
  return (
    <Table aria-label="Networks">
      <TableHeader>
        <TableRow>
          <TableHead>Network</TableHead>
          <TableHead>IP address</TableHead>
          <TableHead>Gateway</TableHead>
          <TableHead>MAC address</TableHead>
          <TableHead>Aliases</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {nets.map(([name, n]) => (
          <TableRow key={name}>
            <TableCell className="font-medium">{name}</TableCell>
            <TableCell className="font-mono text-xs">{n.IPAddress || "—"}</TableCell>
            <TableCell className="font-mono text-xs">{n.Gateway || "—"}</TableCell>
            <TableCell className="font-mono text-xs">{n.MacAddress || "—"}</TableCell>
            <TableCell>{n.Aliases?.join(", ") || "—"}</TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}
