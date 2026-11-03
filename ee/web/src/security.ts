import { queryOptions, useQueryClient } from "@tanstack/react-query";
import { apiFetch } from "@/api/client";
import type {
  EnvBody,
  GlobalBody,
  ImageBadge,
  ImageDetail,
  RowState,
  ScannerStatus,
  ScanNowBody,
  ScanQueued,
  SecuritySettings,
  SecuritySummary,
  Severity,
} from "@/api/gen/types.gen";
import { useSignal } from "@/live/signal";

const BASE = "/api/v1/ee/security";
const json = (method: string, body: unknown) => ({ method, body: JSON.stringify(body), contentType: "application/json" });

export const securityKeys = { all: ["ee", "security"] as const };

export interface ImagesFilter {
  envs: string[];
  running: boolean;
  fixable: boolean;
}

export const imagesQuery = (f: ImagesFilter) =>
  queryOptions({
    queryKey: [...securityKeys.all, "images", f] as const,
    queryFn: () => {
      const q = new URLSearchParams({ running: String(f.running), fixable: String(f.fixable) });
      if (f.envs.length) q.set("envs", f.envs.join(","));
      return apiFetch<SecuritySummary>(`${BASE}/images?${q}`);
    },
  });

export const imageQuery = (image: string) =>
  queryOptions({
    queryKey: [...securityKeys.all, "image", image] as const,
    queryFn: () => apiFetch<ImageDetail>(`${BASE}/image?${new URLSearchParams({ image })}`),
  });

export const envImageQuery = (env: string, id: string) =>
  queryOptions({
    queryKey: [...securityKeys.all, "env-image", env, id] as const,
    queryFn: () => apiFetch<ImageDetail>(`${BASE}/envs/${encodeURIComponent(env)}/images/${encodeURIComponent(id)}`),
  });

export const badgesQuery = (env: string) =>
  queryOptions({
    queryKey: [...securityKeys.all, "badges", env] as const,
    queryFn: () => apiFetch<ImageBadge[]>(`${BASE}/envs/${encodeURIComponent(env)}/badges`),
  });

export const settingsQuery = queryOptions({
  queryKey: [...securityKeys.all, "settings"] as const,
  queryFn: () => apiFetch<SecuritySettings>(`${BASE}/settings`),
});

export const putSettings = (b: GlobalBody) => apiFetch<SecuritySettings>(`${BASE}/settings`, json("PUT", b));
export const putEnvSettings = (env: string, b: EnvBody) =>
  apiFetch<SecuritySettings>(`${BASE}/envs/${encodeURIComponent(env)}/settings`, json("PUT", b));
export const scanNow = (b: ScanNowBody) => apiFetch<ScanQueued>(`${BASE}/scan`, json("POST", b));

/** Refetches every security query whenever the server signals a change. */
export function useSecurityLive() {
  const qc = useQueryClient();
  useSignal("security", () => void qc.invalidateQueries({ queryKey: securityKeys.all }));
}

export const STATE_LABEL: Record<RowState, string> = {
  scanned: "Scanned",
  queued: "Queued",
  scanning: "Scanning…",
  stale: "Stale",
  never_scanned: "Not scanned",
  scanner_unavailable: "Scanner unavailable",
  failed: "Scan failed",
};

export const STATE_TONE: Record<RowState, "run" | "wait" | "fail" | "idle"> = {
  scanned: "run",
  queued: "wait",
  scanning: "wait",
  stale: "wait",
  never_scanned: "idle",
  scanner_unavailable: "fail",
  failed: "fail",
};

export const SEVERITY_LABEL: Record<Severity, string> = {
  critical: "Critical",
  high: "High",
  medium: "Medium",
  low: "Low",
  unknown: "Unknown",
};

export function scannerLabel(s: ScannerStatus): string {
  switch (s.state) {
    case "off":
      return "Off";
    case "preparing":
      return "Preparing…";
    case "ready":
      return "Ready";
    case "unavailable":
      return `Unavailable: ${s.reason}`;
  }
}

export interface SecuritySearch {
  envs?: string;
  running?: boolean;
  fixable?: boolean;
}

/** `?running=false` and `?fixable=false` turn the filters off; anything else keeps them on. */
export const securitySearch = (s: Record<string, unknown>): SecuritySearch => ({
  envs: typeof s.envs === "string" && s.envs ? s.envs : undefined,
  running: s.running === false || s.running === "false" ? false : undefined,
  fixable: s.fixable === false || s.fixable === "false" ? false : undefined,
});
