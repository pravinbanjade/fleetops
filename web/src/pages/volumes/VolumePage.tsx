import { useQuery } from "@tanstack/react-query";
import { Link, useNavigate, useParams } from "@tanstack/react-router";
import { useState } from "react";
import { containersQuery, volumeQuery } from "@/api/queries";
import { Details, DetailError } from "@/components/Details";
import { JsonView } from "@/components/JsonView";
import { Page } from "@/components/Page";
import { useEnvAccess } from "@/api/access";
import { Button } from "@/components/ui/button";
import { Masked } from "@/components/Masked";
import { formatKeyValues } from "@/lib/kv";
import { isSecretOption, maskOptions } from "@/lib/secrets";
import { volumeUsers } from "@/lib/usage";
import { RemoveVolumesDialog } from "./dialogs";

export function VolumePage() {
  const { env, name } = useParams({ from: "/authed/shell/envs/$env/volumes/$name" });
  const { can } = useEnvAccess(env);
  const navigate = useNavigate();
  const q = useQuery(volumeQuery(env, name));
  const containers = useQuery(containersQuery(env));
  const [removing, setRemoving] = useState(false);
  const back = (
    <Link to="/envs/$env/volumes" params={{ env }} className="underline">
      Back to volumes
    </Link>
  );
  if (q.error) return <DetailError error={q.error} what="volume" back={back} />;
  const v = q.data;
  if (!v) return <p className="p-6 text-sm text-muted-foreground">Loading…</p>;
  const users = volumeUsers(containers.data).get(v.Name) ?? [];
  return (
    <Page
      title={v.Name}
      actions={
        can("volumes.delete") && (
          <Button variant="destructive" onClick={() => setRemoving(true)}>
            Remove
          </Button>
        )
      }
    >
      <Details
        label="Volume details"
        items={[
          ["Driver", v.Driver],
          ["Scope", v.Scope],
          ["Mount point", <code key="mp" className="text-xs">{v.Mountpoint}</code>],
          ["Created", v.CreatedAt ? new Date(v.CreatedAt).toLocaleString() : undefined],
          [
            "Used by",
            users.length ? (
              <span key="users" className="flex flex-wrap gap-x-2">
                {users.map((u) => (
                  <Link key={u.id} to="/envs/$env/containers/$id" params={{ env, id: u.id }} className="underline">
                    {u.name}
                  </Link>
                ))}
              </span>
            ) : (
              "no container"
            ),
          ],
          [
            "Options",
            Object.keys(v.Options ?? {}).length ? <OptionList key="options" options={v.Options!} /> : undefined,
          ],
          ["Labels", formatKeyValues(v.Labels) || undefined],
        ]}
      />
      <section className="grid gap-2">
        <h2 className="font-medium">Inspect</h2>
        <JsonView value={v} masked={v.Options ? { ...v, Options: maskOptions(v.Options) } : undefined} label="Volume JSON" />
      </section>
      {removing && (
        <RemoveVolumesDialog
          env={env}
          names={[v.Name]}
          onDone={(removed) => {
            setRemoving(false);
            if (removed) void navigate({ to: "/envs/$env/volumes", params: { env } });
          }}
        />
      )}
    </Page>
  );
}

function OptionList({ options }: { options: Record<string, string> }) {
  return (
    <ul className="grid gap-0.5 font-mono text-xs">
      {Object.entries(options).map(([k, v]) => (
        <li key={k} className="break-all">
          {k}={isSecretOption(k, v) ? <Masked name={k} value={v} /> : v}
        </li>
      ))}
    </ul>
  );
}
