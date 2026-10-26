import type { Catalog } from "./roles";

const GROUPS: [string, string][] = [
  ["docker", "Docker"],
  ["kubernetes", "Kubernetes"],
  ["swarm", "Docker Swarm"],
];

/**
 * A role as the grid: a row per resource type, a column per action, a box
 * only where the row has that action. With `limit` (no license), only boxes
 * already ticked there can change, and only to unticked.
 */
export function RoleGrid({
  catalog,
  value,
  onChange,
  limit,
  readOnly,
}: {
  catalog: Catalog;
  value: string[];
  onChange?: (permissions: string[]) => void;
  limit?: string[];
  readOnly?: boolean;
}) {
  const has = new Set(value);
  const open = (p: string) => !readOnly && (!limit || limit.includes(p) || has.has(p));
  const set = (ps: string[], on: boolean) => {
    const next = new Set(has);
    for (const p of ps.filter(open)) {
      if (on) next.add(p);
      else next.delete(p);
    }
    onChange?.([...next].sort());
  };
  /** Ticks every open box in `ps`, or clears them all when they were all ticked. */
  const toggle = (ps: string[]) => set(ps, !ps.filter(open).every((p) => has.has(p)));
  const column = (verb: string) => catalog.rows.filter((r) => r.cells.includes(verb)).map((r) => `${r.key}.${verb}`);
  const warnings = catalog.rows
    .filter((r) => !has.has(`${r.key}.view`) && r.cells.some((v) => v !== "view" && has.has(`${r.key}.${v}`)))
    .map((r) => `Users won't see ${r.label} in lists.`);
  if (has.has("k8s-other.create")) warnings.push("Creating RBAC objects (Everything else) can give a service account cluster-admin.");
  return (
    <div className="grid gap-3">
      <div className="overflow-x-auto rounded-md bg-[#1c1c1c]">
        <table className="w-full text-sm">
          <thead>
            <tr className="border-b bg-muted/40">
              <th className="px-3 py-2 text-left font-medium">
                <span className="sr-only">Resource</span>
              </th>
              {catalog.verbs.map((v) => (
                <th key={v.key} className="px-2 py-2 text-center font-medium">
                  {readOnly ? (
                    v.label
                  ) : (
                    <button type="button" aria-label={`Toggle ${v.label} for every row`} className="underline-offset-2 hover:underline" onClick={() => toggle(column(v.key))}>
                      {v.label}
                    </button>
                  )}
                </th>
              ))}
            </tr>
          </thead>
          {GROUPS.map(([group, label]) => (
            <tbody key={group}>
              <tr>
                <th colSpan={catalog.verbs.length + 1} className="bg-muted/20 px-3 py-1.5 text-left text-xs font-semibold text-muted-foreground">
                  {label}
                </th>
              </tr>
              {catalog.rows
                .filter((r) => r.group === group)
                .map((r) => (
                  <tr key={r.key} className="border-t">
                    <th scope="row" className="px-3 py-1.5 text-left font-normal">
                      {readOnly ? (
                        r.label
                      ) : (
                        <button type="button" aria-label={`Toggle every action on ${r.label}`} className="text-left underline-offset-2 hover:underline" onClick={() => toggle(r.cells.map((v) => `${r.key}.${v}`))}>
                          {r.label}
                        </button>
                      )}
                    </th>
                    {catalog.verbs.map((v) => {
                      const p = `${r.key}.${v.key}`;
                      return (
                        <td key={v.key} className="px-2 py-1.5 text-center">
                          {r.cells.includes(v.key) && (
                            <input
                              type="checkbox"
                              aria-label={`${r.label}: ${v.label}`}
                              checked={has.has(p)}
                              disabled={!open(p)}
                              onChange={(e) => set([p], e.target.checked)}
                            />
                          )}
                        </td>
                      );
                    })}
                  </tr>
                ))}
            </tbody>
          ))}
        </table>
      </div>
      <p className="text-xs text-muted-foreground">Shell can read mounted secrets. Reveal shows secret values: container env vars and files, Helm values, Secret values.</p>
      {warnings.length > 0 && (
        <ul className="grid gap-1 text-sm text-state-wait-text">
          {warnings.map((w) => (
            <li key={w}>{w}</li>
          ))}
        </ul>
      )}
    </div>
  );
}
