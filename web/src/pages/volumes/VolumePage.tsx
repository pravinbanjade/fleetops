import { useQuery } from "@tanstack/react-query";
import { Link, useNavigate, useParams } from "@tanstack/react-router";
import { useState } from "react";
import { containersQuery, volumeQuery } from "@/api/queries";
import { Details, DetailError } from "@/components/Details";
import { JsonView } from "@/components/JsonView";
import { Page } from "@/components/Page";
import { Button } from "@/components/ui/button";
import { formatKeyValues } from "@/lib/kv";
import { volumeUsers } from "@/lib/usage";
import { RemoveVolumesDialog } from "./dialogs";

export function VolumePage() {
  const { env, name } = useParams({ from: "/authed/shell/envs/$env/volumes/$name" });
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
        <Button variant="destructive" onClick={() => setRemoving(true)}>
          Remove
        </Button>
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
          ["Options", formatKeyValues(v.Options) || undefined],
          ["Labels", formatKeyValues(v.Labels) || undefined],
        ]}
      />
      <section className="grid gap-2">
        <h2 className="font-medium">Inspect</h2>
        <JsonView value={v} label="Volume JSON" />
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
