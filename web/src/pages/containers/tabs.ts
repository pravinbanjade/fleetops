/** The container page's tabs, in order; the router validates `?tab=` against them. */
export const CONTAINER_TABS = ["overview", "inspect", "env", "mounts", "networks", "logs", "terminal", "files"] as const;
export type ContainerTab = (typeof CONTAINER_TABS)[number];
