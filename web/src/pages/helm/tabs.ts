export const HELM_TABS = ["overview", "history", "values", "manifest", "notes"] as const;
export type HelmTab = (typeof HELM_TABS)[number];
