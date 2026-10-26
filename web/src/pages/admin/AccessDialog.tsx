import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { NativeSelect } from "@/components/ui/native-select";
import { Label } from "@/components/ui/label";
import { toast } from "sonner";
import { revokeUserAccess, setUserAccess } from "@/api/gen/sdk.gen";
import type { AccessEntry, EnvView, User } from "@/api/gen/types.gen";
import { envsQuery, keys, userAccessQuery } from "@/api/queries";
import { Slot } from "@/ee-slots";
import { errorMessage } from "@/lib/errors";
import { accessBody, EnvRow, fromEntry, NONE, same, type Row } from "./EnvAccessRow";
import { FormDialog, LoadingDialog } from "./FormDialog";

/** What `user` may do in each environment; saves only the rows that changed. */
export function AccessDialog({ user, onClose }: { user: User; onClose: () => void }) {
  const qc = useQueryClient();
  const envs = useQuery(envsQuery);
  const access = useQuery(userAccessQuery(user.id));
  if (!envs.data || !access.data)
    return <LoadingDialog title={`Access for ${user.username}`} error={envs.error ?? access.error} onClose={onClose} />;
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
        const changed = envs.filter((e) => !same(rows[e.id], initial[e.id]));
        const failed = await applyEach(changed, (e) => {
          const row = rows[e.id];
          const path = { id: user.id, env_id: e.id };
          return row.role === "none"
            ? revokeUserAccess({ path, throwOnError: true })
            : setUserAccess({ path, body: accessBody(row), throwOnError: true });
        }, (e) => e.name);
        await onSaved();
        if (failed) throw new Error(failed);
        const revoked = changed.filter((e) => rows[e.id].role === "none").map((e) => e.name);
        toast.success(
          `Saved access for ${user.username}` + (revoked.length ? `; removed from ${revoked.join(", ")}` : ""),
        );
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

/**
 * Runs `f` on every item, not stopping at a failure; returns null when all
 * succeeded, or which ones failed and why, naming the ones that saved.
 */
async function applyEach<T>(items: T[], f: (t: T) => Promise<unknown>, name: (t: T) => string): Promise<string | null> {
  const failed: string[] = [];
  for (const it of items) {
    try {
      await f(it);
    } catch (e) {
      failed.push(`${name(it)} (${errorMessage(e)})`);
    }
  }
  if (!failed.length) return null;
  const saved = items.length - failed.length;
  return `Not saved for ${failed.join(", ")}.` + (saved ? ` The other ${saved} saved.` : "");
}

/** One environment's role, given to several users at once. */
export function BulkAccessDialog({ users, onClose }: { users: User[]; onClose: () => void }) {
  const qc = useQueryClient();
  const envs = useQuery(envsQuery);
  const [envId, setEnvId] = useState<number | null>(null);
  const [row, setRow] = useState<Row>({ role: "viewer", namespaces: null });
  if (!envs.data) return <LoadingDialog title="Grant access" error={envs.error} onClose={onClose} />;
  const env: EnvView | undefined = envs.data.find((e) => e.id === envId) ?? envs.data[0];
  const names = users.map((u) => u.username).join(", ");
  return (
    <FormDialog
      title={`Access for ${users.length} users`}
      description={`Sets one environment's role for ${names}. Their access elsewhere is unchanged.`}
      submitLabel={row.role === "none" ? "Remove access" : "Grant access"}
      onClose={onClose}
      onSubmit={async () => {
        if (!env) throw new Error("Add an environment first");
        if (row.namespaces && row.namespaces.length === 0) throw new Error(`Add at least one namespace, or choose the whole environment`);
        const failed = await applyEach(users, (u) => {
          const path = { id: u.id, env_id: env.id };
          return row.role === "none"
            ? revokeUserAccess({ path, throwOnError: true })
            : setUserAccess({ path, body: accessBody(row), throwOnError: true });
        }, (u) => u.username);
        await Promise.all([
          ...users.map((u) => qc.invalidateQueries({ queryKey: keys.userAccess(u.id) })),
          qc.invalidateQueries({ queryKey: keys.envs }),
        ]);
        if (failed) throw new Error(failed);
        toast.success(`${row.role === "none" ? "Removed" : "Granted"} ${env.name} access for ${users.length} users`);
      }}
    >
      {env ? (
        <>
          <div className="grid gap-2">
            <Label htmlFor="bulk-env">Environment</Label>
            <NativeSelect
              id="bulk-env"
              value={env.id}
              onChange={(e) => {
                setEnvId(Number(e.target.value));
                setRow({ ...NONE, role: "viewer" });
              }}
            >
              {envs.data.map((e) => (
                <option key={e.id} value={e.id}>
                  {e.name}
                </option>
              ))}
            </NativeSelect>
          </div>
          <EnvRow key={env.id} env={env} row={row} onChange={setRow} />
        </>
      ) : (
        <p className="text-sm text-muted-foreground">No environments yet.</p>
      )}
    </FormDialog>
  );
}
