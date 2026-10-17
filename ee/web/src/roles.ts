import { queryOptions } from "@tanstack/react-query";
import { apiFetch } from "@/api/client";

export interface RoleSummary {
  id: number;
  name: string;
  description: string | null;
  /** `row.verb`, sorted. */
  permissions: string[];
  users: number;
  teams: number;
}

export interface RoleHolder {
  id: number;
  name: string;
  env: string;
}

export interface RoleDetail {
  id: number;
  name: string;
  description: string | null;
  permissions: string[];
  holders: { users: RoleHolder[]; teams: RoleHolder[] };
}

/** The role grid: each row lists the verbs it has a cell for. */
export interface Catalog {
  rows: { key: string; label: string; group: string; cells: string[] }[];
  verbs: { key: string; label: string }[];
}

const URL = "/api/v1/ee/roles";
const json = (v: unknown) => ({ body: JSON.stringify(v), contentType: "application/json" });

export const roleKeys = {
  all: ["ee", "roles"] as const,
  role: (id: number) => ["ee", "roles", id] as const,
  catalog: ["ee", "roles", "catalog"] as const,
};

export const rolesQuery = queryOptions({ queryKey: roleKeys.all, queryFn: () => apiFetch<RoleSummary[]>(URL) });
export const roleQuery = (id: number) => queryOptions({ queryKey: roleKeys.role(id), queryFn: () => apiFetch<RoleDetail>(`${URL}/${id}`) });
export const catalogQuery = queryOptions({ queryKey: roleKeys.catalog, queryFn: () => apiFetch<Catalog>(`${URL}/catalog`), staleTime: Infinity });

export const createRole = (name: string, description: string | null, permissions: string[]) =>
  apiFetch<RoleDetail>(URL, { method: "POST", ...json({ name, description, permissions }) });
export const updateRole = (id: number, change: { name?: string; description?: string | null; permissions?: string[] }) =>
  apiFetch<RoleDetail>(`${URL}/${id}`, { method: "PATCH", ...json(change) });
export const deleteRole = (id: number) => apiFetch<void>(`${URL}/${id}`, { method: "DELETE" });

/** What the built-in roles allow, for showing them in the grid (the server's fixed sets). */
export function builtinPermissions(role: "viewer" | "operator", catalog: Catalog): string[] {
  const all = catalog.rows.flatMap((r) => r.cells.map((v) => `${r.key}.${v}`));
  if (role === "viewer") return all.filter((p) => p.endsWith(".view") || p === "containers.reveal" || p === "helm.reveal");
  return all.filter((p) => p !== "secrets.reveal");
}
