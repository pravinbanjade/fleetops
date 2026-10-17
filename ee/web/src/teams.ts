import { queryOptions } from "@tanstack/react-query";
import { apiFetch } from "@/api/client";
import type { AccessEntry, EnvRole, User } from "@/api/gen/types.gen";

export interface TeamSummary {
  id: number;
  name: string;
  description: string | null;
  members: number;
  environments: number;
}

export interface TeamDetail {
  id: number;
  name: string;
  description: string | null;
  members: User[];
  access: AccessEntry[];
}

/** Where part of a user's access comes from; `team_id` null is a direct grant. */
export interface AccessSource {
  team_id: number | null;
  team_name: string | null;
  role: EnvRole;
  namespaces: string[] | null;
}

export interface EffectiveAccess {
  env_id: number;
  env_name: string;
  env_kind: string;
  role: EnvRole;
  namespaces: string[] | null;
  sources: AccessSource[];
}

const URL = "/api/v1/ee/teams";
const json = (v: unknown) => ({ body: JSON.stringify(v), contentType: "application/json" });

export const teamKeys = {
  all: ["ee", "teams"] as const,
  team: (id: number) => ["ee", "teams", id] as const,
  effective: ["ee", "effective-access"] as const,
};

export const teamsQuery = queryOptions({
  queryKey: teamKeys.all,
  queryFn: () => apiFetch<TeamSummary[]>(URL),
});

export const teamQuery = (id: number) =>
  queryOptions({ queryKey: teamKeys.team(id), queryFn: () => apiFetch<TeamDetail>(`${URL}/${id}`) });

export const userEffectiveAccessQuery = (userId: number) =>
  queryOptions({
    queryKey: [...teamKeys.effective, userId] as const,
    queryFn: () => apiFetch<EffectiveAccess[]>(`/api/v1/ee/users/${userId}/access`),
  });

export const createTeam = (name: string, description: string | null) =>
  apiFetch<TeamDetail>(URL, { method: "POST", ...json({ name, description }) });
export const updateTeam = (id: number, name: string, description: string | null) =>
  apiFetch<TeamDetail>(`${URL}/${id}`, { method: "PATCH", ...json({ name, description }) });
export const deleteTeam = (id: number) => apiFetch<void>(`${URL}/${id}`, { method: "DELETE" });
export const addTeamMember = (id: number, userId: number) =>
  apiFetch<void>(`${URL}/${id}/members/${userId}`, { method: "PUT" });
export const removeTeamMember = (id: number, userId: number) =>
  apiFetch<void>(`${URL}/${id}/members/${userId}`, { method: "DELETE" });
export const setTeamAccess = (id: number, envId: number, role: EnvRole, namespaces: string[] | null) =>
  apiFetch<AccessEntry>(`${URL}/${id}/access/${envId}`, {
    method: "PUT",
    ...json({ role, ...(namespaces ? { namespaces } : {}) }),
  });
export const revokeTeamAccess = (id: number, envId: number) =>
  apiFetch<void>(`${URL}/${id}/access/${envId}`, { method: "DELETE" });
