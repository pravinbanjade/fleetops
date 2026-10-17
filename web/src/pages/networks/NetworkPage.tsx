import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate, useParams } from "@tanstack/react-router";
import { useState, type FormEvent } from "react";
import { containersQuery, keys, networkQuery } from "@/api/queries";
import { connectNetwork, disconnectNetwork } from "@/api/gen/sdk.gen";
import type { NetworkInspect } from "@/api/docker";
import { Details, DetailError } from "@/components/Details";
import { JsonView } from "@/components/JsonView";
import { Page } from "@/components/Page";
import { useEnvAccess } from "@/api/access";
import { Button } from "@/components/ui/button";
import { NativeSelect } from "@/components/ui/native-select";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { errorMessage } from "@/lib/errors";
import { formatKeyValues } from "@/lib/kv";
import { FormError } from "@/pages/AuthLayout";
import { BUILTIN_NETWORKS, RemoveNetworksDialog } from "./dialogs";

export function NetworkPage() {
  const { env, id } = useParams({ from: "/authed/shell/envs/$env/networks/$id" });
  const { can } = useEnvAccess(env);
  const navigate = useNavigate();
  const q = useQuery(networkQuery(env, id));
  const [removing, setRemoving] = useState(false);
  const back = (
    <Link to="/envs/$env/networks" params={{ env }} className="underline">
      Back to networks
    </Link>
  );
  if (q.error) return <DetailError error={q.error} what="network" back={back} />;
  const n = q.data;
  if (!n) return <p className="p-6 text-sm text-muted-foreground">Loading…</p>;
  const ipam = n.IPAM?.Config ?? [];
  return (
    <Page
      title={n.Name}
      actions={
        can("networks.delete") &&
        !BUILTIN_NETWORKS.includes(n.Name) && (
          <Button variant="destructive" onClick={() => setRemoving(true)}>
            Remove
          </Button>
        )
      }
    >
      <Details
        label="Network details"
        items={[
          ["Driver", n.Driver],
          ["Scope", n.Scope],
          ["Subnet", ipam.map((c) => c.Subnet).filter(Boolean).join(", ")],
          ["Gateway", ipam.map((c) => c.Gateway).filter(Boolean).join(", ")],
          ["Internal", n.Internal ? "yes" : "no"],
          ["Attachable", n.Attachable ? "yes" : "no"],
          ["Options", formatKeyValues(n.Options) || undefined],
          ["Labels", formatKeyValues(n.Labels) || undefined],
        ]}
      />
      <Attached env={env} n={n} />
      <section className="grid gap-2">
        <h2 className="font-medium">Inspect</h2>
        <JsonView value={n} label="Network JSON" />
      </section>
      {removing && (
        <RemoveNetworksDialog
          env={env}
          networks={[{ id: n.Id, name: n.Name }]}
          onDone={(removed) => {
            setRemoving(false);
            if (removed) void navigate({ to: "/envs/$env/networks", params: { env } });
          }}
        />
      )}
    </Page>
  );
}

/** The attached containers, with disconnect, and a picker to connect another. */
function Attached({ env, n }: { env: string; n: NetworkInspect }) {
  const qc = useQueryClient();
  const containers = useQuery(containersQuery(env));
  const attached = Object.entries(n.Containers ?? {});
  const others = (containers.data ?? []).filter((c) => !n.Containers?.[c.id]);
  const [pick, setPick] = useState("");
  const canWrite = useEnvAccess(env).can("networks.operate");
  const refresh = () => qc.invalidateQueries({ queryKey: keys.networks(env) });
  const connect = useMutation({
    meta: { inline: true },
    mutationFn: (container: string) => connectNetwork({ path: { env, id: n.Id }, body: { container } }),
    onSuccess: () => setPick(""),
    onSettled: refresh,
  });
  const disconnect = useMutation({
    meta: { inline: true },
    mutationFn: (container: string) => disconnectNetwork({ path: { env, id: n.Id }, body: { container } }),
    onSettled: refresh,
  });
  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (pick) connect.mutate(pick);
  };
  const error = connect.error ?? disconnect.error;
  return (
    <section className="grid gap-3">
      <h2 className="font-medium">Containers</h2>
      {attached.length === 0 ? (
        <p className="text-sm text-muted-foreground">No container is attached.</p>
      ) : (
        <Table aria-label="Attached containers">
          <TableHeader>
            <TableRow>
              <TableHead>Container</TableHead>
              <TableHead>IPv4 address</TableHead>
              <TableHead>MAC address</TableHead>
              <TableHead>
                <span className="sr-only">Actions</span>
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {attached.map(([cid, c]) => (
              <TableRow key={cid}>
                <TableCell>
                  <Link to="/envs/$env/containers/$id" params={{ env, id: cid }} className="font-medium hover:underline">
                    {c.Name}
                  </Link>
                </TableCell>
                <TableCell className="font-mono text-xs">{c.IPv4Address || "—"}</TableCell>
                <TableCell className="font-mono text-xs">{c.MacAddress || "—"}</TableCell>
                <TableCell className="text-right">
                  {canWrite && (
                    <Button
                      variant="outline"
                      size="sm"
                      disabled={disconnect.isPending}
                      onClick={() => disconnect.mutate(cid)}
                    >
                      Disconnect
                    </Button>
                  )}
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      )}
      {canWrite && (
        <form onSubmit={submit} className="flex max-w-md items-center gap-2">
          <NativeSelect aria-label="Container to connect" value={pick} onChange={(e) => setPick(e.target.value)}>
            <option value="">Choose a container…</option>
            {others.map((c) => (
              <option key={c.id} value={c.id}>
                {c.name}
              </option>
            ))}
          </NativeSelect>
          <Button type="submit" variant="outline" disabled={!pick || connect.isPending}>
            Connect
          </Button>
        </form>
      )}
      <FormError message={error ? errorMessage(error) : null} />
    </section>
  );
}
