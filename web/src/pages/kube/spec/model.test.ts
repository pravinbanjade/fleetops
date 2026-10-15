import { parse } from "yaml";
import { describe, expect, it } from "vitest";
import { activeTweaks, applyOps, readSpec, TWEAKS } from "./model";

const deployment = `apiVersion: apps/v1
kind: Deployment
metadata:
  name: api
  labels:
    app.kubernetes.io/managed-by: Helm
spec:
  template:
    spec:
      initContainers:
        - name: migrate
          image: api:1
      containers:
        - name: api
          image: api:1
          env:
            - name: LOG_LEVEL
              value: info
            - name: DATABASE_URL
              valueFrom:
                secretKeyRef: { name: db, key: url }
          envFrom:
            - configMapRef: { name: api-config }
              prefix: CFG_
`;

const configMap = `kind: ConfigMap
metadata:
  name: api-config
data:
  LOG_LEVEL: info
  app.properties: |
    a=1
binaryData:
  logo: aGVsbG8=
`;

const tweaksOf = (text: string) => {
  const raw = (parse(text) as { metadata: { annotations?: Record<string, string> } }).metadata.annotations?.[TWEAKS];
  return raw ? JSON.parse(raw) : undefined;
};

describe("reading an object's spec", () => {
  it("lists containers, their env, where each value comes from, and envFrom", () => {
    const s = readSpec("deployments", deployment);
    expect(s.containers?.map((c) => [c.name, c.init])).toEqual([["migrate", true], ["api", false]]);
    const api = s.containers![1];
    expect(api.env.map((e) => e.source)).toEqual([
      { kind: "value", value: "info" },
      { kind: "secrets", name: "db", key: "url" },
    ]);
    expect(api.envFrom).toEqual([{ kind: "configmaps", name: "api-config", prefix: "CFG_" }]);
    expect(s.managedBy).toBe("Helm");
  });

  it("lists data keys in order, binary ones by size, and a Secret's values as hidden", () => {
    expect(readSpec("configmaps", configMap).data).toEqual([
      { key: "LOG_LEVEL", value: "info" },
      { key: "app.properties", value: "a=1\n" },
      { key: "logo", value: null, bytes: 6 },
    ]);
    expect(readSpec("secrets", "kind: Secret\ndata:\n  url: ••••\n").data).toEqual([{ key: "url", value: null }]);
  });
});

describe("tweaking env and data", () => {
  it("changes, adds and removes env vars and records what they replaced", () => {
    const out = applyOps(
      "deployments",
      deployment,
      [
        { op: "env", container: "api", name: "LOG_LEVEL", to: { value: "debug" } },
        { op: "env", container: "api", name: "DEBUG", to: { value: "1" } },
        { op: "env", container: "api", name: "DATABASE_URL", to: null },
      ],
      "ana",
      1_000_000,
    );
    const api = readSpec("deployments", out).containers![1];
    expect(api.env.map((e) => [e.name, e.spec])).toEqual([
      ["LOG_LEVEL", { value: "debug" }],
      ["DEBUG", { value: "1" }],
    ]);
    expect(tweaksOf(out)).toEqual({
      by: "ana",
      at: 1000,
      env: { api: { LOG_LEVEL: { value: "info" }, DEBUG: null, DATABASE_URL: { valueFrom: { secretKeyRef: { name: "db", key: "url" } } } } },
    });
    expect(activeTweaks(readSpec("deployments", out))).toHaveLength(3);
  });

  it("keeps the first value across tweaks, and drops the record once everything is back", () => {
    const once = applyOps("deployments", deployment, [{ op: "env", container: "api", name: "LOG_LEVEL", to: { value: "debug" } }]);
    const twice = applyOps("deployments", once, [{ op: "env", container: "api", name: "LOG_LEVEL", to: { value: "trace" } }]);
    expect(tweaksOf(twice).env.api.LOG_LEVEL).toEqual({ value: "info" });
    const back = applyOps("deployments", twice, activeTweaks(readSpec("deployments", twice)));
    expect(tweaksOf(back)).toBeUndefined();
    expect(readSpec("deployments", back).containers![1].env[0].spec).toEqual({ value: "info" });
  });

  it("reverts a ConfigMap key it added by removing it", () => {
    const out = applyOps("configmaps", configMap, [{ op: "data", key: "NEW", to: "x" }, { op: "data", key: "LOG_LEVEL", to: "debug" }]);
    expect(tweaksOf(out).data).toEqual({ NEW: null, LOG_LEVEL: "info" });
    const back = applyOps("configmaps", out, activeTweaks(readSpec("configmaps", out)));
    expect(readSpec("configmaps", back).data?.map((d) => d.key)).toEqual(["LOG_LEVEL", "app.properties", "logo"]);
  });

  it("writes a Secret's new value as stringData and records nothing", () => {
    const out = applyOps("secrets", "kind: Secret\nmetadata:\n  name: db\ndata:\n  url: ••••\n  old: ••••\n", [
      { op: "data", key: "url", to: "postgres://x" },
      { op: "data", key: "old", to: null },
    ]);
    const v = parse(out);
    expect(v.data).toEqual({});
    expect(v.stringData).toEqual({ url: "postgres://x" });
    expect(tweaksOf(out)).toBeUndefined();
  });

  it("quotes every value it writes, so YAML 1.1 never reads one as a bool or number", () => {
    const out = applyOps("configmaps", configMap, [{ op: "data", key: "TRACE", to: "on" }, { op: "data", key: "MODE", to: "0644" }]);
    expect(out).toContain('TRACE: "on"');
    expect(out).toContain('MODE: "0644"');
    const env = applyOps("deployments", deployment, [{ op: "env", container: "api", name: "DEBUG", to: { value: "yes" } }]);
    expect(env).toContain('value: "yes"');
  });

  it("says when the container has gone", () => {
    expect(() => applyOps("deployments", deployment, [{ op: "env", container: "gone", name: "X", to: null }])).toThrow(/gone/);
  });
});
