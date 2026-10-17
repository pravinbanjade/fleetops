import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { toast } from "sonner";
import { revokeUserAccess, setUserAccess } from "@/api/gen/sdk.gen";
import type { AccessEntry, EnvView, User } from "@/api/gen/types.gen";
import { envsQuery, keys, userAccessQuery } from "@/api/queries";
import { Slot } from "@/ee-slots";
import { accessBody, EnvRow, fromEntry, same, type Row } from "./EnvAccessRow";
import { FormDialog } from "./FormDialog";

/** What `user` may do in each environment; saves only the rows that changed. */
export function AccessDialog({ user, onClose }: { user: User; onClose: () => void }) {
  const qc = useQueryClient();
  const envs = useQuery(envsQuery);
  const access = useQuery(userAccessQuery(user.id));
  if (!envs.data || !access.data) return null;
  return <AccessForm user={user} envs={envs.data} loaded={access.data} onClose={onClose} onSaved={() => Promise.all([qc.invalidateQueries({ queryKey: keys.userAccess(user.id) }), qc.invalidateQueries({ queryKey: keys.envs })])} />;
}

function AccessForm({
  user,
  envs,
  loaded,
  onClose,
  onSaved,
}: {
  user: User;
  envs: EnvView[];
  loaded: AccessEntry[];
  onClose: () => void;
  onSaved: () => Promise<unknown>;
}) {
  const initial = Object.fromEntries(envs.map((e) => [e.id, fromEntry(loaded.find((a) => a.env_id === e.id))]));
  const [rows, setRows] = useState<Record<number, Row>>(initial);
  const set = (id: number, row: Row) => setRows((r) => ({ ...r, [id]: row }));
  return (
    <FormDialog
      title={`Access for ${user.username}`}
      description="Admins manage environments; this user works only where you grant a role."
      submitLabel="Save"
      onClose={onClose}
      onSubmit={async () => {
        for (const e of envs) {
          const row = rows[e.id];
          if (row.namespaces && row.namespaces.length === 0) {
            throw new Error(`Add at least one namespace on ${e.name}, or choose Entire cluster`);
          }
        }
        for (const e of envs) {
          const row = rows[e.id];
          if (same(row, initial[e.id])) continue;
          const path = { id: user.id, env_id: e.id };
          if (row.role === "none") await revokeUserAccess({ path, throwOnError: true });
          else await setUserAccess({ path, body: accessBody(row), throwOnError: true });
        }
        await onSaved();
        toast.success(`Saved access for ${user.username}`);
      }}
    >
      {envs.length === 0 && <p className="text-sm text-muted-foreground">No environments yet.</p>}
      <div className="grid gap-3">
        {envs.map((e) => (
          <EnvRow key={e.id} env={e} row={rows[e.id]} onChange={(r) => set(e.id, r)} />
        ))}
      </div>
      <Slot name="user.access" props={{ userId: user.id }} />
    </FormDialog>
  );
}
