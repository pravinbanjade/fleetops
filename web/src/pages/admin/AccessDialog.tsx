import { useQuery, useQueryClient } from "@tanstack/react-query";
import { X } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import { revokeUserAccess, setUserAccess } from "@/api/gen/sdk.gen";
import type { AccessEntry, EnvRole, EnvView, User } from "@/api/gen/types.gen";
import { envsQuery, keys, userAccessQuery } from "@/api/queries";
import { Input } from "@/components/ui/input";
import { NativeSelect } from "@/components/ui/native-select";
import { isKube } from "@/lib/env";
import { FormDialog } from "./FormDialog";

/** One environment's row in the dialog: its role, and for a cluster, the namespaces. */
interface Row {
  role: EnvRole | "none";
  /** Null: the whole environment. */
  namespaces: string[] | null;
}

const NONE: Row = { role: "none", namespaces: null };

function fromEntry(e: AccessEntry | undefined): Row {
  return e ? { role: e.role, namespaces: e.namespaces ?? null } : NONE;
}

const same = (a: Row, b: Row) => a.role === b.role && JSON.stringify(a.namespaces) === JSON.stringify(b.namespaces);

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
          else await setUserAccess({ path, body: { role: row.role, ...(row.namespaces ? { namespaces: row.namespaces } : {}) }, throwOnError: true });
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
    </FormDialog>
  );
}

function EnvRow({ env, row, onChange }: { env: EnvView; row: Row; onChange: (r: Row) => void }) {
  const [typed, setTyped] = useState("");
  const kube = isKube(env.kind);
  const add = () => {
    const names = typed
      .split(/[\s,]+/)
      .map((n) => n.trim())
      .filter(Boolean);
    if (!names.length) return;
    onChange({ ...row, namespaces: [...new Set([...(row.namespaces ?? []), ...names])].sort() });
    setTyped("");
  };
  return (
    <div role="group" aria-label={env.name} className="grid gap-2 rounded-md border p-3">
      <div className="flex items-center justify-between gap-3">
        <span className="text-sm font-medium">{env.name}</span>
        <NativeSelect
          aria-label={`Role on ${env.name}`}
          value={row.role}
          onChange={(e) => onChange({ role: e.target.value as Row["role"], namespaces: e.target.value === "none" ? null : row.namespaces })}
        >
          <option value="none">None</option>
          <option value="viewer">Viewer</option>
          <option value="operator">Operator</option>
        </NativeSelect>
      </div>
      {kube && row.role !== "none" && (
        <>
          <label className="flex items-center gap-2 text-sm">
            <input type="checkbox" checked={row.namespaces === null} onChange={(e) => onChange({ ...row, namespaces: e.target.checked ? null : [] })} />
            Entire cluster
          </label>
          {row.namespaces !== null && (
            <div className="grid gap-2">
              <div className="flex flex-wrap gap-1">
                {row.namespaces.map((n) => (
                  <span key={n} className="flex items-center gap-1 rounded border px-1.5 py-0.5 font-mono text-xs">
                    {n}
                    <button type="button" aria-label={`Remove ${n}`} onClick={() => onChange({ ...row, namespaces: row.namespaces!.filter((x) => x !== n) })}>
                      <X className="size-3" />
                    </button>
                  </span>
                ))}
              </div>
              <Input
                aria-label={`Namespaces on ${env.name}`}
                placeholder="Type a namespace, then Enter"
                value={typed}
                onChange={(e) => setTyped(e.target.value)}
                onBlur={add}
                onKeyDown={(e) => {
                  if (e.key === "Enter" || e.key === ",") {
                    e.preventDefault();
                    add();
                  }
                }}
              />
            </div>
          )}
        </>
      )}
    </div>
  );
}
