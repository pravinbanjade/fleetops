import { describe, expect, it } from "vitest";
import { envSummary } from "./summary";

describe("envSummary", () => {
  it("names a Docker host with its containers", () => {
    expect(envSummary("docker", { containers: 8 })).toBe("Docker · 8 containers");
  });

  it("uses the singular for one", () => {
    expect(envSummary("docker", { containers: 1 })).toBe("Docker · 1 container");
  });

  it("falls back to the kind alone when the count is unknown", () => {
    expect(envSummary("kube", {})).toBe("Kubernetes");
  });

  it("counts a swarm's nodes", () => {
    expect(envSummary("swarm", { nodes: 3 })).toBe("Swarm · 3 nodes");
  });

  it("counts a cluster's pods", () => {
    expect(envSummary("kube", { pods: 41 })).toBe("Kubernetes · 41 pods");
  });
});
