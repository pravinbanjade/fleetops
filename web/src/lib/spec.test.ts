import { describe, expect, it } from "vitest";
import type { ContainerSpec } from "@/api/gen/types.gen";
import { joinArgs, splitArgs } from "./args";
import { EMPTY_FORM, formToSpec, forDuplicate, specToForm } from "./spec";

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
    { container_port: 80, host_port: 8080, host_ip: "127.0.0.1", protocol: "tcp" },
    { container_port: 53, host_port: null, host_ip: null, protocol: "udp" },
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
    expect(() => formToSpec({ ...base, ports: [{ host: "80", container: "http", hostIp: "", protocol: "tcp" }] })).toThrow(
      'Container port must be a number from 1 to 65535, not "http"',
    );
    expect(() => formToSpec({ ...base, ports: [{ host: "70000", container: "80", hostIp: "", protocol: "tcp" }] })).toThrow(
      "Host port must be",
    );
    expect(() => formToSpec({ ...base, labels: "novalue" })).toThrow('Label "novalue" must look like key=value');
    expect(() => formToSpec({ ...base, memoryMb: "lots" })).toThrow('Memory must be a positive number, not "lots"');
    expect(() => formToSpec({ ...base, advanced: "{nope" })).toThrow("Advanced settings are not valid JSON");
    expect(() => formToSpec({ ...base, advanced: "[1]" })).toThrow("Advanced settings must be a JSON object");
    expect(() => formToSpec({ ...base, cmd: `echo "x` })).toThrow('Command: unclosed " quote');
  });

  it("skips empty port rows", () => {
    const f = { ...EMPTY_FORM, image: "redis", ports: [{ host: "", hostIp: "", container: "", protocol: "tcp" as const }] };
    expect(formToSpec(f).ports).toEqual([]);
  });

  it("carries multi-line and padded values unchanged", () => {
    const spec: ContainerSpec = {
      ...SPEC,
      cmd: ["sh", "-c", "echo a\necho b"],
      env: ["A= padded ", "CERT=-----BEGIN\nabc\n-----END"],
      labels: { tier: "front", note: "line 1\nline 2" },
    };
    const form = specToForm(spec);
    expect(form.env).toBe("A= padded ");
    expect(form.labels).toBe("tier=front");
    expect(formToSpec(form)).toEqual(spec);
  });
});

describe("forDuplicate", () => {
  it("drops what would make the copy share the original's identity", () => {
    const anon = "0f3e9a".padEnd(64, "0");
    const copy = forDuplicate({
      ...SPEC,
      labels: { tier: "front", "com.docker.compose.project": "app", "com.docker.compose.service": "db" },
      advanced: {
        Hostname: "db",
        User: "999",
        HostConfig: {
          ShmSize: 1024,
          Mounts: [
            { Type: "volume", Source: anon, Target: "/cache" },
            { Type: "volume", Source: "pgdata", Target: "/var/lib/postgresql/data" },
          ],
        },
        NetworkingConfig: { EndpointsConfig: { app_default: { Aliases: ["db"], IPAMConfig: { IPv4Address: "10.0.0.5" } } } },
      },
    });
    expect(copy.labels).toEqual({ tier: "front" });
    expect(copy.ports?.map((p) => p.host_port)).toEqual([null, null]);
    expect(copy.ports?.[0].host_ip).toBe("127.0.0.1");
    expect(copy.advanced).toEqual({
      User: "999",
      HostConfig: { ShmSize: 1024, Mounts: [{ Type: "volume", Source: "pgdata", Target: "/var/lib/postgresql/data" }] },
    });
  });
});
