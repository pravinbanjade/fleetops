import { useQuery } from "@tanstack/react-query";
import type { EnvRole, Grant } from "@/api/gen/types.gen";
import { envsQuery } from "./queries";

export interface EnvAccess {
  role: EnvRole;
  canWrite: boolean;
  /** Null: the whole environment. */
  namespaces: string[] | null;
}

/** The caller's access from an environment row; no field means full access (the server enforces either way). */
export function accessOf(env: { access?: Grant | null } | undefined): EnvAccess {
  const role = env?.access?.role ?? "operator";
  return { role, canWrite: role === "operator", namespaces: env?.access?.namespaces ?? null };
}

export function useEnvAccess(env?: string): EnvAccess {
  const { data } = useQuery(envsQuery);
  return accessOf(data?.find((e) => e.name === env));
}
