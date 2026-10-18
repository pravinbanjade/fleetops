import { queryOptions } from "@tanstack/react-query";
import { apiFetch } from "@/api/client";

export type SsoKind = "microsoft" | "google" | "github" | "oidc" | "ldap";

export type LdapPreset = "active_directory" | "openldap" | "custom";

/** A connection's settings, tagged by `kind`; secrets are never part of it. */
export type SsoSettings =
  | { kind: "microsoft"; tenant: string; client_id: string }
  | { kind: "google"; client_id: string; hosted_domains: string[]; admin_email?: string | null }
  | { kind: "github"; client_id: string; orgs: string[]; base_url?: string | null }
  | { kind: "oidc"; issuer: string; client_id: string; scopes: string[]; groups_claim: string }
  | {
      kind: "ldap";
      preset: LdapPreset;
      url: string;
      starttls: boolean;
      skip_tls_verify: boolean;
      ca_cert?: string | null;
      bind_dn?: string | null;
      user_base_dn: string;
      user_filter: string;
      username_attr: string;
      email_attr: string;
      group_base_dn?: string | null;
      group_filter: string;
    };

export interface SsoSecrets {
  client_secret?: string;
  service_account_json?: string;
  bind_password?: string;
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
export const testConnection = (id: number, who: { email?: string; username?: string } = {}) =>
  apiFetch<TestResult>(`${URL}/connections/${id}/test`, { method: "POST", ...json(who) });
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
  ldap: "LDAP / Active Directory",
};

export const PRESET_LABEL: Record<LdapPreset, string> = {
  active_directory: "Active Directory",
  openldap: "OpenLDAP",
  custom: "Custom",
};

/** Each preset's user filter, username attribute and group filter (the server's defaults). */
export const LDAP_PRESETS: Record<LdapPreset, { user_filter: string; username_attr: string; group_filter: string }> = {
  active_directory: {
    user_filter: "(&(objectClass=user)(sAMAccountName={username}))",
    username_attr: "sAMAccountName",
    group_filter: "(member:1.2.840.113556.1.4.1941:={user_dn})",
  },
  openldap: {
    user_filter: "(&(objectClass=inetOrgPerson)(uid={username}))",
    username_attr: "uid",
    group_filter: "(|(&(objectClass=groupOfNames)(member={user_dn}))(&(objectClass=posixGroup)(memberUid={username})))",
  },
  custom: { user_filter: "", username_attr: "", group_filter: "" },
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
    case "ldap":
      return {
        kind,
        preset: "active_directory",
        url: "",
        starttls: false,
        skip_tls_verify: false,
        ca_cert: null,
        bind_dn: null,
        user_base_dn: "",
        email_attr: "mail",
        group_base_dn: null,
        ...LDAP_PRESETS.active_directory,
      };
  }
}
