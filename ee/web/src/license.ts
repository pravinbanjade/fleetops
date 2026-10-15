import { queryOptions } from "@tanstack/react-query";
import { apiFetch } from "@/api/client";
import type { Feature } from "@/api/gen/types.gen";

export type LicenseStatus = "none" | "invalid" | "valid" | "grace" | "expired";

export interface LicenseInfo {
  status: LicenseStatus;
  reason: string | null;
  id: string | null;
  customer: string | null;
  expires_at: number | null;
  max_users: number | null;
  features: Feature[];
  users_in_use: number;
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
      return exp - now < GRACE_SECS ? `The Enterprise license expires on ${day(exp)}.` : null;
    default:
      return null;
  }
}
