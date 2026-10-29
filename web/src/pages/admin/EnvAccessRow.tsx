import { X } from "lucide-react";
import { useState } from "react";
import type { AccessEntry, EnvRole, EnvView, SetAccess } from "@/api/gen/types.gen";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { NativeSelect } from "@/components/ui/native-select";
import { useRoleOptions } from "@/ee-slots";
import { isKube, isSwarm } from "@/lib/env";

/** One environment's row in the dialog: its role, and for a cluster, the namespaces. */
export interface Row {
  /** A built-in role, `custom:<id>` for a custom role (Enterprise), or none. */
  role: EnvRole | "none" | `custom:${number}`;
  /** Null: the whole environment. */
  namespaces: string[] | null;
}

export const NONE: Row = { role: "none", namespaces: null };

export function fromEntry(e: AccessEntry | undefined): Row {
  if (!e) return NONE;
  return { role: e.role_id != null ? `custom:${e.role_id}` : e.role, namespaces: e.namespaces ?? null };
}

/** What to send to store `row` (not "none"). */
export function accessBody(row: Row): SetAccess {
  const ns = row.namespaces ? { namespaces: row.namespaces } : {};
  return row.role.startsWith("custom:") ? { role_id: Number(row.role.slice(7)), ...ns } : { role: row.role as EnvRole, ...ns };
}

export const same = (a: Row, b: Row) => a.role === b.role && JSON.stringify(a.namespaces) === JSON.stringify(b.namespaces);

const RANK: Record<string, number> = { none: 0, viewer: 1, operator: 2 };

/**
 * One environment's role, and for a cluster its namespaces. With `limit`,
 * the row can only be lowered or narrowed from it.
 */
export function EnvRow({ env, row, onChange, limit }: { env: EnvView; row: Row; onChange: (r: Row) => void; limit?: Row }) {
  const roles = useRoleOptions();
  // Frozen (`limit`): a built-in no higher than the old one, or the same custom role.
  const above = (role: Row["role"]) => {
    if (limit === undefined || role === limit.role) return false;
    if (role.startsWith("custom:") || limit.role.startsWith("custom:")) return role !== "none";
    return RANK[role] > RANK[limit.role];
  };
  const listOnly = limit !== undefined && limit.namespaces !== null;
  const [typed, setTyped] = useState("");
  const kube = isKube(env.kind);
  // A swarm grant can be limited to stacks, as a cluster's to namespaces.
  const swarm = isSwarm(env.kind);
  const scope = swarm ? { all: "Entire swarm", what: "Stacks", one: "stack" } : { all: "Entire cluster", what: "Namespaces", one: "namespace" };
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
    <div role="group" aria-label={env.name} className="grid gap-2 rounded-md bg-raised p-3">
      <div className="flex items-center justify-between gap-3">
        <span className="text-sm font-medium">{env.name}</span>
        <NativeSelect
          aria-label={`Role on ${env.name}`}
          value={row.role}
          onChange={(e) => onChange({ role: e.target.value as Row["role"], namespaces: e.target.value === "none" ? null : row.namespaces })}
        >
          <option value="none">None</option>
          <option value="viewer" disabled={above("viewer")}>
            Viewer
          </option>
          <option value="operator" disabled={above("operator")}>
            Operator
          </option>
          {roles.map((r) => (
            <option key={r.id} value={`custom:${r.id}`} disabled={above(`custom:${r.id}`)}>
              {r.name}
            </option>
          ))}
        </NativeSelect>
      </div>
      {(kube || swarm) && row.role !== "none" && (
        <>
          <label className="flex items-center gap-2 text-sm">
            <Checkbox
              checked={row.namespaces === null}
              disabled={listOnly && row.namespaces !== null}
              onCheckedChange={(v) => onChange({ ...row, namespaces: v === true ? null : [] })}
            />
            {scope.all}
          </label>
          {row.namespaces !== null && (
            <div className="grid gap-2">
              <div className="flex flex-wrap gap-1">
                {row.namespaces.map((n) => (
                  <span key={n} className="flex items-center gap-1 rounded border px-1.5 py-0.5 font-mono text-xs">
                    {n}
                    <button
                      type="button"
                      aria-label={`Remove ${n}`}
                      className="-my-1 -mr-1.5 grid size-6 place-items-center rounded-xs text-muted-foreground hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                      onClick={() => onChange({ ...row, namespaces: row.namespaces!.filter((x) => x !== n) })}
                    >
                      <X aria-hidden className="size-3" />
                    </button>
                  </span>
                ))}
              </div>
              {!listOnly && (
                <Input
                  aria-label={`${scope.what} on ${env.name}`}
                  placeholder={`Type a ${scope.one}, then Enter`}
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
              )}
            </div>
          )}
        </>
      )}
    </div>
  );
}
