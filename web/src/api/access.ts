import { findEnv } from "@/lib/env";
import { useQuery } from "@tanstack/react-query";
import type { EnvRole } from "@/api/gen/types.gen";
import { envsQuery } from "./queries";

/** One of the caller's grants on an environment: where it applies and what it allows. */
export interface GrantHint {
  /** Null: the whole environment. */
  namespaces?: string[] | null;
  permissions: string[];
}

export interface EnvAccess {
  role: EnvRole;
  /** Null: the whole environment. */
  namespaces: string[] | null;
  /** The custom role's name when the caller holds exactly one. */
  roleName: string | null;
  /**
   * Whether some grant allows `permission` (e.g. `volumes.delete`): in namespace
   * `ns`; with `null`, across the whole environment; omitted, anywhere.
   */
  can: (permission: string, ns?: string | null) => boolean;
}

/** What a built-in role allows, for a response without `grants` (the server's fixed sets). */
function builtin(role: EnvRole, permission: string): boolean {
  if (role === "viewer")
    return (
      permission.endsWith(".view") ||
      permission === "containers.reveal" ||
      permission === "helm.reveal" ||
      permission === "stacks.reveal"
    );
  return permission !== "secrets.reveal";
}

type AccessField = { role: EnvRole; role_name?: string | null; namespaces?: string[] | null; grants?: GrantHint[] } | null;

/** The caller's access from an environment row; no field means full access (the server enforces either way). */
export function accessOf(env: { access?: AccessField } | undefined): EnvAccess {
  const access = env?.access;
  const grants = access?.grants;
  const can = (permission: string, ns?: string | null) => {
    if (!access) return true;
    if (!grants) return builtin(access.role, permission);
    return grants.some((g) => {
      if (!g.permissions.includes(permission)) return false;
      const set = g.namespaces ?? null;
      if (ns === undefined) return true;
      if (ns === null) return set === null;
      return set === null || set.includes(ns);
    });
  };
  return { role: access?.role ?? "operator", roleName: access?.role_name ?? null, namespaces: access?.namespaces ?? null, can };
}

export function useEnvAccess(env?: string): EnvAccess {
  const { data } = useQuery(envsQuery);
  return accessOf(findEnv(data, env));
}
