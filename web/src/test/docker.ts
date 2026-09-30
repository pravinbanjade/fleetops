import type { ContainerSummary } from "@/api/gen/types.gen";

/** A container list entry: running nginx, created two hours ago, unless overridden. */
export function container(over: Partial<ContainerSummary> & { id: string; name: string }): ContainerSummary {
  return {
    image: "nginx:1.27",
    image_id: "sha256:img",
    created: Math.floor(Date.now() / 1000) - 7200,
    labels: {},
    ports: [],
    state: "running",
    status: "Up 2 hours",
    volumes: [],
    ...over,
  };
}
