import { queryOptions } from "@tanstack/react-query";
import { apiFetch } from "@/api/client";
import type { StagedBackup } from "@/api/gen/types.gen";

export type Retention = { kind: "count"; value: number } | { kind: "days"; value: number };

export interface BackupSchedule {
  enabled: boolean;
  hour_utc: number;
  retention: Retention;
  passphrase_set: boolean;
}

export type TargetKind = "s3" | "local";

export type TargetSettings =
  | {
      kind: "s3";
      endpoint?: string | null;
      region: string;
      bucket: string;
      prefix: string;
      path_style: boolean;
      access_key_id: string;
    }
  | { kind: "local"; path: string };

export interface BackupTarget {
  kind: TargetKind;
  settings: TargetSettings;
  secret_set: boolean;
}

export interface BackupRun {
  id: number;
  started_at: number;
  finished_at: number | null;
  trigger: "schedule" | "manual" | "catch_up";
  target_kind: TargetKind | null;
  result: "success" | "failure" | "skipped";
  file_name: string | null;
  size: number | null;
  error: string | null;
}

export interface BackupStatus {
  licensed: boolean;
  enabled: boolean;
  paused: boolean;
  last_run: BackupRun | null;
  last_failed: boolean;
  next_run_at: number | null;
}

export interface RecentBackup {
  target_kind: TargetKind;
  name: string;
  size: number;
  created_at: number;
}

export interface RecentBackups {
  backups: RecentBackup[];
  errors: { target_kind: TargetKind; error: string }[];
}

const BASE = "/api/v1/ee/backup";
const json = (method: string, body: unknown) => ({ method, body: JSON.stringify(body), contentType: "application/json" });

export const backupKeys = {
  all: ["ee", "backup"] as const,
  status: ["ee", "backup", "status"] as const,
};

export const scheduleQuery = queryOptions({
  queryKey: ["ee", "backup", "schedule"] as const,
  queryFn: () => apiFetch<BackupSchedule>(`${BASE}/schedule`),
});
export const targetsQuery = queryOptions({
  queryKey: ["ee", "backup", "targets"] as const,
  queryFn: () => apiFetch<BackupTarget[]>(`${BASE}/targets`),
});
export const runsQuery = queryOptions({
  queryKey: ["ee", "backup", "runs"] as const,
  queryFn: () => apiFetch<BackupRun[]>(`${BASE}/runs?limit=10`),
});
export const statusQuery = queryOptions({
  queryKey: backupKeys.status,
  queryFn: () => apiFetch<BackupStatus>(`${BASE}/status`),
});
export const recentQuery = queryOptions({
  queryKey: ["ee", "backup", "recent"] as const,
  queryFn: () => apiFetch<RecentBackups>(`${BASE}/recent`),
});

export const putSchedule = (s: Omit<BackupSchedule, "passphrase_set">) => apiFetch<BackupSchedule>(`${BASE}/schedule`, json("PUT", s));
export const putPassphrase = (passphrase: string) => apiFetch<void>(`${BASE}/schedule/passphrase`, json("PUT", { passphrase }));
export const putTarget = (settings: TargetSettings, secret_access_key?: string) =>
  apiFetch<BackupTarget>(`${BASE}/targets/${settings.kind}`, json("PUT", { settings, secret_access_key }));
export const deleteTarget = (kind: TargetKind) => apiFetch<void>(`${BASE}/targets/${kind}`, { method: "DELETE" });
export const testTarget = (kind: TargetKind) => apiFetch<void>(`${BASE}/targets/${kind}/test`, { method: "POST" });
export const runNow = () => apiFetch<void>(`${BASE}/run`, { method: "POST" });
export const stageRecent = (target_kind: TargetKind, name: string, passphrase?: string) =>
  apiFetch<StagedBackup>(`${BASE}/recent/restore`, json("POST", { target_kind, name, passphrase }));

/** `hour_utc`:00 in the viewer's time, given their offset east of UTC in minutes. */
export function localTimeOf(hourUtc: number, offsetMinutes: number): string {
  const m = (((hourUtc * 60 + offsetMinutes) % 1440) + 1440) % 1440;
  return `${String(Math.floor(m / 60)).padStart(2, "0")}:${String(m % 60).padStart(2, "0")}`;
}

export const viewerOffset = () => -new Date().getTimezoneOffset();

/** What the admin banner says, or null. */
export function backupBanner(s: BackupStatus): string | null {
  if (s.paused) return "Scheduled backups paused: license required";
  if (s.last_failed) {
    const r = s.last_run;
    if (!r) return "The last scheduled backup failed.";
    const when = new Date((r.finished_at ?? r.started_at) * 1000).toISOString().slice(0, 16).replace("T", " ");
    return `The scheduled backup at ${when} UTC failed${r.error ? `: ${r.error}` : "."}`;
  }
  return null;
}
