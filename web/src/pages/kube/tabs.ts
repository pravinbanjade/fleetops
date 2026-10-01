export const KUBE_TABS = ["summary", "yaml", "events", "logs", "terminal"] as const;
export type KubeTab = (typeof KUBE_TABS)[number];
