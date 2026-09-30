import type { ContainerSummary, ImageSummary } from "@/api/gen/types.gen";
import type { ContainerInspect } from "@/api/docker";

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

/** Docker's inspect body for a container, trimmed to what the UI reads. */
export function inspect(over: Partial<ContainerInspect> & { Id: string; Name: string }): ContainerInspect {
  return {
    Created: "2026-09-30T10:00:00Z",
    State: { Status: "running", Running: true, StartedAt: "2026-09-30T10:00:01Z", ExitCode: 0 },
    Config: { Image: "nginx:1.27", Env: [], Cmd: ["nginx", "-g", "daemon off;"], Labels: {} },
    HostConfig: { RestartPolicy: { Name: "unless-stopped" } },
    Mounts: [],
    NetworkSettings: { Ports: {}, Networks: {} },
    ...over,
  };
}

export function image(over: Partial<ImageSummary> & { id: string }): ImageSummary {
  return { repo_tags: [], size: 50_000_000, created: Math.floor(Date.now() / 1000) - 86400, in_use: false, ...over };
}
