import { queryOptions } from "@tanstack/react-query";
import { apiFetch } from "@/api/client";
import type { Feature } from "@/api/gen/types.gen";

export type LicenseStatus = "none" | "invalid" | "valid" | "grace" | "expired";

export interface LicenseInfo {
  status: LicenseStatus;
  /** Uploaded in Admin → License, or read from `--license-file`; null without a license. */
  source: "uploaded" | "file" | null;
  reason: string | null;
  id: string | null;
  customer: string | null;
  expires_at: number | null;
  max_users: number | null;
  features: Feature[];
  users_in_use: number;
  /** Docker or Podman hosts plus Kubernetes nodes; null: unlimited. */
  max_nodes: number | null;
  nodes_in_use: number;
}

export const GRACE_SECS = 14 * 86_400;

const URL = "/api/v1/ee/license";

export const licenseQuery = queryOptions({
  queryKey: ["ee", "license"] as const,
  queryFn: () => apiFetch<LicenseInfo>(URL),
});

export const uploadLicense = (text: string) =>
  apiFetch<LicenseInfo>(URL, { method: "PUT", body: text, contentType: "text/plain" });
export const removeLicense = () => apiFetch<LicenseInfo>(URL, { method: "DELETE" });

/** What a pasted license says about itself, read without checking the signature (the server does that on install). */
export interface LicensePeek {
  id: string;
  customer: string;
  expires_at: number;
  max_users: number | null;
  max_nodes: number | null;
  features: string[];
}

/** The unverified payload of `text`, or null when it isn't shaped like a license. */
export function peekLicense(text: string): LicensePeek | null {
  const payload = text.trim().split(".")[0];
  if (!payload) return null;
  try {
    const b64 = payload.replace(/-/g, "+").replace(/_/g, "/");
    const bytes = Uint8Array.from(atob(b64.padEnd(Math.ceil(b64.length / 4) * 4, "=")), (c) => c.charCodeAt(0));
    const p = JSON.parse(new TextDecoder().decode(bytes));
    if (typeof p?.customer !== "string" || typeof p?.expires_at !== "number" || !Array.isArray(p?.features)) return null;
    return {
      id: String(p.id ?? ""),
      customer: p.customer,
      expires_at: p.expires_at,
      max_users: p.max_users ?? null,
      max_nodes: p.max_nodes ?? null,
      features: p.features.map(String),
    };
  } catch {
    return null;
  }
}

const day = (secs: number) => new Date(secs * 1000).toISOString().slice(0, 10);

/** What the admin banner says about `l` at `now` (unix seconds), or null. */
export function bannerFor(l: LicenseInfo, now: number): string | null {
  const exp = l.expires_at ?? 0;
  switch (l.status) {
    case "invalid":
      return `The Enterprise license is not valid (${l.reason ?? "unknown reason"}). Enterprise features are off.`;
    case "expired":
      return "The Enterprise license has expired. Enterprise features are off.";
    case "grace":
      return `The Enterprise license expired on ${day(exp)}. Enterprise features turn off on ${day(exp + GRACE_SECS)}.`;
    case "valid":
      if (l.max_nodes !== null && l.nodes_in_use > l.max_nodes)
        return `FleetOps manages ${l.nodes_in_use} nodes; the license covers ${l.max_nodes}. New environments can't be added until you add nodes to the license.`;
      return exp - now < GRACE_SECS ? `The Enterprise license expires on ${day(exp)}.` : null;
    default:
      return null;
  }
}
