import { describe, expect, it } from "vitest";
import { formFromSpec, specFromForm } from "./serviceSpec";

const SPEC = {
  Name: "blog_web",
  Labels: { "com.docker.stack.namespace": "blog" },
  TaskTemplate: {
    ContainerSpec: { Image: "nginx:1.27", Env: ["MODE=prod"], Secrets: [{ SecretID: "sec1", SecretName: "blog_db" }] },
    Resources: { Limits: { MemoryBytes: 1024 } },
    Placement: { Constraints: ["node.role == worker"] },
  },
  Mode: { Replicated: { Replicas: 3 } },
  EndpointSpec: { Ports: [{ Protocol: "udp", TargetPort: 53, PublishedPort: 5353 }] },
};

describe("service spec form", () => {
  it("round trips the form fields", () => {
    const form = formFromSpec(SPEC);
    expect(form).toEqual({ name: "blog_web", image: "nginx:1.27", mode: "replicated", replicas: 3, env: "MODE=prod", ports: "5353:53/udp", constraints: "node.role == worker" });
    expect(formFromSpec(specFromForm(form, SPEC))).toEqual(form);
  });

  it("keeps fields the form does not show", () => {
    const spec = specFromForm({ ...formFromSpec(SPEC), image: "nginx:1.28" }, SPEC);
    expect(spec.Labels).toEqual(SPEC.Labels);
    expect(spec.TaskTemplate.ContainerSpec.Secrets).toEqual(SPEC.TaskTemplate.ContainerSpec.Secrets);
    expect(spec.TaskTemplate.Resources).toEqual(SPEC.TaskTemplate.Resources);
    expect(spec.TaskTemplate.ContainerSpec.Image).toBe("nginx:1.28");
    expect(SPEC.TaskTemplate.ContainerSpec.Image).toBe("nginx:1.27");
  });

  it("global drops Replicated", () => {
    expect(specFromForm({ ...formFromSpec(SPEC), mode: "global" }, SPEC).Mode).toEqual({ Global: {} });
  });

  it("parses ports with and without protocol", () => {
    const spec = specFromForm({ ...formFromSpec(SPEC), ports: "8080:80\n5353:53/udp" });
    expect(spec.EndpointSpec.Ports).toEqual([
      { PublishedPort: 8080, TargetPort: 80, Protocol: "tcp" },
      { PublishedPort: 5353, TargetPort: 53, Protocol: "udp" },
    ]);
  });

  it("rejects a bad port line", () => {
    expect(() => specFromForm({ ...formFromSpec(SPEC), ports: "80" })).toThrow("line 1: use published:target or published:target/udp");
  });

  it("leaves untouched fields exactly as they were", () => {
    const base = {
      Name: "dns",
      TaskTemplate: { ContainerSpec: { Image: "coredns:1", Env: ["PAD= x "] } },
      Mode: { ReplicatedJob: { MaxConcurrent: 1 } },
      EndpointSpec: { Ports: [{ Protocol: "udp", TargetPort: 53, PublishedPort: 53, PublishMode: "host", Name: "dns" }] },
    };
    const spec = specFromForm({ ...formFromSpec(base), image: "coredns:2" }, base);
    expect(spec.EndpointSpec).toEqual(base.EndpointSpec);
    expect(spec.TaskTemplate.ContainerSpec.Env).toEqual(["PAD= x "]);
    expect(spec.Mode).toEqual(base.Mode);
    expect(spec.TaskTemplate.ContainerSpec.Image).toBe("coredns:2");
  });
});
