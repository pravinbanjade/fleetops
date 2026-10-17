/** The paid plans, as https://fleetops.pravinb.com.np/pricing lists them. */

export const PRICING_URL = "https://fleetops.pravinb.com.np/pricing";
export const SALES_URL = "https://fleetops.pravinb.com.np/contact#sales";

export interface Plan {
  name: string;
  /** US dollars before tax; null when priced per contract. */
  yearly: number | null;
  monthly: number | null;
  nodes: string;
  support: string;
  features: string[];
  cta: { label: string; href: string };
  recommended?: boolean;
}

export const PLANS: Plan[] = [
  {
    name: "Starter",
    yearly: 99,
    monthly: 9,
    nodes: "10 to 30 nodes",
    support: "GitHub issues, Enterprise label",
    features: ["Teams with shared environment access", "Audit export (CSV, NDJSON)", "Offline license, 14-day grace after expiry"],
    cta: { label: "Subscribe", href: PRICING_URL },
  },
  {
    name: "Scale",
    yearly: 199,
    monthly: 19,
    nodes: "10 to 50 nodes",
    support: "Email, next business day",
    features: ["Everything in Starter", "Onboarding call", "Invoice and purchase-order billing"],
    cta: { label: "Subscribe", href: PRICING_URL },
    recommended: true,
  },
  {
    name: "Enterprise",
    yearly: null,
    monthly: null,
    nodes: "More than 50 nodes",
    support: "Priority email, agreed response times",
    features: ["Everything in Scale", "Nodes added mid-term", "Custom license terms", "Security questionnaire answered"],
    cta: { label: "Talk to us", href: SALES_URL },
  },
];
