import { queryOptions } from "@tanstack/react-query";
import { apiFetch } from "@/api/client";

export type SsoKind = "microsoft" | "google" | "github" | "oidc";

/** A connection's settings, tagged by `kind`; secrets are never part of it. */
export type SsoSettings =
  | { kind: "microsoft"; tenant: string; client_id: string }
  | { kind: "google"; client_id: string; hosted_domains: string[]; admin_email?: string | null }
  | { kind: "github"; client_id: string; orgs: string[]; base_url?: string | null }
  | { kind: "oidc"; issuer: string; client_id: string; scopes: string[]; groups_claim: string };

export interface SsoSecrets {
  client_secret?: string;
  service_account_json?: string;
}

export interface SsoConnection {
  id: number;
  name: string;
  kind: SsoKind;
  enabled: boolean;
  settings: SsoSettings;
  /** Names of the secrets that are saved. */
  secrets_set: string[];
  require_mapped_group: boolean;
  session_hours: number;
  /** Null until FLEETOPS_PUBLIC_URL is set. */
  callback_url: string | null;
  user_count: number;
}

export interface SsoMapping {
  id: number;
  group: string;
  team_id: number | null;
  team_name: string | null;
  admin: boolean;
}

export type PasswordLogin = "everyone" | "local_admins";

export interface TestResult {
  ok: boolean;
  detail: string;
  groups: string[] | null;
}

const URL = "/api/v1/ee/sso";
const json = (v: unknown) => ({ body: JSON.stringify(v), contentType: "application/json" });

export const ssoKeys = {
  all: ["ee", "sso"] as const,
  mappings: (id: number) => ["ee", "sso", id, "mappings"] as const,
  settings: ["ee", "sso", "settings"] as const,
};

export const connectionsQuery = queryOptions({
  queryKey: ssoKeys.all,
  queryFn: () => apiFetch<SsoConnection[]>(`${URL}/connections`),
});
export const mappingsQuery = (id: number) =>
  queryOptions({ queryKey: ssoKeys.mappings(id), queryFn: () => apiFetch<SsoMapping[]>(`${URL}/connections/${id}/mappings`) });
export const ssoSettingsQuery = queryOptions({
  queryKey: ssoKeys.settings,
  queryFn: () => apiFetch<{ password_login: PasswordLogin }>(`${URL}/settings`),
});

export interface ConnectionInput {
  name: string;
  settings: SsoSettings;
  secrets: SsoSecrets;
  require_mapped_group: boolean;
  session_hours: number;
}

export const createConnection = (c: ConnectionInput) =>
  apiFetch<SsoConnection>(`${URL}/connections`, { method: "POST", ...json(c) });
export const updateConnection = (id: number, change: Partial<ConnectionInput> & { enabled?: boolean }) =>
  apiFetch<SsoConnection>(`${URL}/connections/${id}`, { method: "PATCH", ...json(change) });
export const deleteConnection = (id: number) =>
  apiFetch<{ users_left: number }>(`${URL}/connections/${id}`, { method: "DELETE" });
export const testConnection = (id: number, email?: string) =>
  apiFetch<TestResult>(`${URL}/connections/${id}/test`, { method: "POST", ...json(email ? { email } : {}) });
export const createMapping = (id: number, group: string, target: { team_id: number } | { admin: true }) =>
  apiFetch<SsoMapping>(`${URL}/connections/${id}/mappings`, { method: "POST", ...json({ group, ...target }) });
export const deleteMapping = (id: number, mappingId: number) =>
  apiFetch<void>(`${URL}/connections/${id}/mappings/${mappingId}`, { method: "DELETE" });
export const putSsoSettings = (password_login: PasswordLogin) =>
  apiFetch<{ password_login: PasswordLogin }>(`${URL}/settings`, { method: "PUT", ...json({ password_login }) });

export const KIND_LABEL: Record<SsoKind, string> = {
  microsoft: "Microsoft Entra ID",
  google: "Google Workspace",
  github: "GitHub",
  oidc: "OpenID Connect",
};

/** Comma- or newline-separated text as a list. */
export const list = (text: string) =>
  text
    .split(/[,\n]/)
    .map((s) => s.trim())
    .filter(Boolean);

/** Empty settings for a new connection of `kind`. */
export function blankSettings(kind: SsoKind): SsoSettings {
  switch (kind) {
    case "microsoft":
      return { kind, tenant: "", client_id: "" };
    case "google":
      return { kind, client_id: "", hosted_domains: [], admin_email: null };
    case "github":
      return { kind, client_id: "", orgs: [], base_url: null };
    case "oidc":
      return { kind, issuer: "", client_id: "", scopes: ["openid", "profile", "email"], groups_claim: "groups" };
  }
}
