import { describe, expect, it } from "vitest";
import type { ContainerSpec } from "@/api/gen/types.gen";
import { joinArgs, splitArgs } from "./args";
import { EMPTY_FORM, formToSpec, specToForm } from "./spec";

describe("splitArgs", () => {
  it("splits on whitespace and groups quotes", () => {
    expect(splitArgs(`nginx -g "daemon off;"`)).toEqual(["nginx", "-g", "daemon off;"]);
    expect(splitArgs(`sh -c 'echo "hi there"'`)).toEqual(["sh", "-c", 'echo "hi there"']);
    expect(splitArgs(`a\\ b "" c`)).toEqual(["a b", "", "c"]);
    expect(splitArgs("   ")).toEqual([]);
  });

  it("rejects an unclosed quote", () => {
    expect(() => splitArgs(`echo "oops`)).toThrow('unclosed " quote');
  });

  it("round-trips through joinArgs", () => {
    const args = ["sh", "-c", 'echo "a b" \\ c', ""];
    expect(splitArgs(joinArgs(args))).toEqual(args);
  });
});

const SPEC: ContainerSpec = {
  image: "nginx:1.27",
  name: "web",
  cmd: ["nginx", "-g", "daemon off;"],
  entrypoint: null,
  env: ["A=1", "B=x=y"],
  ports: [
    { container_port: 80, host_port: 8080, protocol: "tcp" },
    { container_port: 53, host_port: null, protocol: "udp" },
  ],
  binds: ["data:/data", "/srv:/srv:ro"],
  network: "shop",
  restart_policy: "unless-stopped",
  labels: { tier: "front", "a.b": "c=d" },
  memory_bytes: 512 * 1024 * 1024,
  nano_cpus: 1_500_000_000,
  cap_add: ["NET_ADMIN", "SYS_TIME"],
  privileged: false,
  advanced: { HostConfig: { ShmSize: 1024 } },
};

describe("container spec form", () => {
  it("round-trips a spec through the form", () => {
    expect(formToSpec(specToForm(SPEC))).toEqual(SPEC);
  });

  it("sends nulls for fields left empty", () => {
    expect(formToSpec({ ...EMPTY_FORM, image: " redis " })).toEqual({
      image: "redis",
      name: null,
      cmd: null,
      entrypoint: null,
      ports: [],
      env: [],
      binds: [],
      network: null,
      restart_policy: "no",
      labels: {},
      memory_bytes: null,
      nano_cpus: null,
      cap_add: [],
      privileged: false,
      advanced: null,
    });
  });

  it("names the field that is wrong", () => {
    const base = { ...EMPTY_FORM, image: "redis" };
    expect(() => formToSpec({ ...base, image: "" })).toThrow("Image is required");
    expect(() => formToSpec({ ...base, ports: [{ host: "80", container: "http", protocol: "tcp" }] })).toThrow(
      'Container port must be a number from 1 to 65535, not "http"',
    );
    expect(() => formToSpec({ ...base, ports: [{ host: "70000", container: "80", protocol: "tcp" }] })).toThrow(
      "Host port must be",
    );
    expect(() => formToSpec({ ...base, labels: "novalue" })).toThrow('Label "novalue" must look like key=value');
    expect(() => formToSpec({ ...base, memoryMb: "lots" })).toThrow('Memory must be a positive number, not "lots"');
    expect(() => formToSpec({ ...base, advanced: "{nope" })).toThrow("Advanced settings are not valid JSON");
    expect(() => formToSpec({ ...base, advanced: "[1]" })).toThrow("Advanced settings must be a JSON object");
    expect(() => formToSpec({ ...base, cmd: `echo "x` })).toThrow('Command: unclosed " quote');
  });

  it("skips empty port rows", () => {
    const f = { ...EMPTY_FORM, image: "redis", ports: [{ host: "", container: "", protocol: "tcp" as const }] };
    expect(formToSpec(f).ports).toEqual([]);
  });
});
