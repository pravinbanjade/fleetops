import { queryOptions } from "@tanstack/react-query";
import { apiFetch } from "@/api/client";
import type {
  DeliveryView,
  DestinationBody,
  DestinationKind,
  DestinationView,
  RuleBody,
  RuleView,
  Severity,
  SmtpBody,
  SmtpView,
} from "@/api/gen/types.gen";

const BASE = "/api/v1/ee/notifications";
const json = (method: string, body: unknown) => ({ method, body: JSON.stringify(body), contentType: "application/json" });

export const notifyKeys = { all: ["ee", "notifications"] as const };

export const destinationsQuery = queryOptions({
  queryKey: [...notifyKeys.all, "destinations"] as const,
  queryFn: () => apiFetch<DestinationView[]>(`${BASE}/destinations`),
});
export const rulesQuery = queryOptions({
  queryKey: [...notifyKeys.all, "rules"] as const,
  queryFn: () => apiFetch<RuleView[]>(`${BASE}/rules`),
});
export const smtpQuery = queryOptions({
  queryKey: [...notifyKeys.all, "smtp"] as const,
  queryFn: () => apiFetch<SmtpView | null>(`${BASE}/smtp`),
});
export const deliveriesQuery = queryOptions({
  queryKey: [...notifyKeys.all, "deliveries"] as const,
  queryFn: () => apiFetch<DeliveryView[]>(`${BASE}/deliveries?limit=50`),
  refetchInterval: 15_000,
});

export const createDestination = (b: DestinationBody) => apiFetch<DestinationView>(`${BASE}/destinations`, json("POST", b));
export const updateDestination = (id: number, b: DestinationBody) => apiFetch<DestinationView>(`${BASE}/destinations/${id}`, json("PUT", b));
export const deleteDestination = (id: number) => apiFetch<void>(`${BASE}/destinations/${id}`, { method: "DELETE" });
export const testDestination = (id: number) => apiFetch<void>(`${BASE}/destinations/${id}/test`, { method: "POST" });
export const createRule = (b: RuleBody) => apiFetch<RuleView>(`${BASE}/rules`, json("POST", b));
export const updateRule = (id: number, b: RuleBody) => apiFetch<RuleView>(`${BASE}/rules/${id}`, json("PUT", b));
export const deleteRule = (id: number) => apiFetch<void>(`${BASE}/rules/${id}`, { method: "DELETE" });
export const putSmtp = (b: SmtpBody) => apiFetch<SmtpView | null>(`${BASE}/smtp`, json("PUT", b));
export const testSmtp = (to: string) => apiFetch<void>(`${BASE}/smtp/test`, json("POST", { to }));

export const KIND_LABEL: Record<DestinationKind, string> = {
  slack: "Slack",
  email: "Email",
  webhook: "Generic webhook",
  pagerduty: "PagerDuty",
  teams: "Microsoft Teams",
  discord: "Discord",
};

export const SEVERITY_LABEL: Partial<Record<Severity, string>> = {
  critical: "Critical",
  high: "High and above",
  medium: "Medium and above",
  low: "Low and above",
};

/** "Critical · fix only · running". */
export function threshold(r: RuleView): string {
  const sev = r.min_severity === "critical" ? "Critical" : `${SEVERITY_LABEL[r.min_severity]?.split(" ")[0]}+`;
  return [sev, r.fixable_only ? "fix only" : null, r.running_only ? "running" : null].filter(Boolean).join(" · ");
}
