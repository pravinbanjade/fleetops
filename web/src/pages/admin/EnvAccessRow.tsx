import { X } from "lucide-react";
import { useState } from "react";
import type { AccessEntry, EnvRole, EnvView } from "@/api/gen/types.gen";
import { Input } from "@/components/ui/input";
import { NativeSelect } from "@/components/ui/native-select";
import { isKube } from "@/lib/env";

/** One environment's row in the dialog: its role, and for a cluster, the namespaces. */
export interface Row {
  role: EnvRole | "none";
  /** Null: the whole environment. */
  namespaces: string[] | null;
}

export const NONE: Row = { role: "none", namespaces: null };

export function fromEntry(e: AccessEntry | undefined): Row {
  return e ? { role: e.role, namespaces: e.namespaces ?? null } : NONE;
}

export const same = (a: Row, b: Row) => a.role === b.role && JSON.stringify(a.namespaces) === JSON.stringify(b.namespaces);

/** One environment's role, and for a cluster its namespaces. */
export function EnvRow({ env, row, onChange }: { env: EnvView; row: Row; onChange: (r: Row) => void }) {
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
