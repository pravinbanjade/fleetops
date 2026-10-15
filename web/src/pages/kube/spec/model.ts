import { isMap, isSeq, parse, parseDocument, Scalar, YAMLMap, YAMLSeq, type Document } from "yaml";
import type { KubeKind } from "@/api/gen/types.gen";

/** Where FleetOps keeps what a tweak replaced, so it can be put back. */
export const TWEAKS = "fleetops.io/tweaks";
const LAST_APPLIED = "kubectl.kubernetes.io/last-applied-configuration";
/** What the server shows in place of a Secret's value. */
const SECRET_MASK = "••••";

/** An env var as the pod spec holds it, without its name. */
export type EnvSpec = { value?: string; valueFrom?: Record<string, unknown> };

export type EnvSource =
  | { kind: "value"; value: string }
  | { kind: "configmaps" | "secrets"; name: string; key: string }
  | { kind: "field"; path: string }
  | { kind: "resource"; resource: string }
  | { kind: "other" };

export type EnvVar = { name: string; spec: EnvSpec; source: EnvSource };
export type EnvFrom = { kind: "configmaps" | "secrets"; name: string; prefix?: string };
export type Container = { name: string; init: boolean; image: string; env: EnvVar[]; envFrom: EnvFrom[] };

/** A ConfigMap or Secret key. `value` is null for a Secret's hidden value; `bytes` is set for binary data. */
export type DataEntry = { key: string; value: string | null; bytes?: number };

/** The values a tweak replaced: null where the key or variable did not exist. */
export type Tweaks = {
  by?: string;
  at: number;
  env?: Record<string, Record<string, EnvSpec | null>>;
  data?: Record<string, string | null>;
};

export type ObjectSpec = {
  labels: [string, string][];
  annotations: [string, string][];
  /** Null when the kind has no pod template. */
  containers: Container[] | null;
  /** Null unless a ConfigMap or Secret. */
  data: DataEntry[] | null;
  immutable: boolean;
  /** `app.kubernetes.io/managed-by`, e.g. Helm. */
  managedBy?: string;
  tweaks: Tweaks | null;
};

export type EnvOp = { op: "env"; container: string; name: string; to: EnvSpec | null };
export type DataOp = { op: "data"; key: string; to: string | null };
export type Op = EnvOp | DataOp;

/** Kinds whose pod template can change in place; a Job's cannot, and Pods and ReplicaSets belong to something else. */
export const ENV_EDITABLE: readonly KubeKind[] = ["deployments", "statefulsets", "daemonsets", "cronjobs"];

function podSpecPath(kind: KubeKind): string[] | null {
  switch (kind) {
    case "pods":
      return ["spec"];
    case "cronjobs":
      return ["spec", "jobTemplate", "spec", "template", "spec"];
    case "deployments":
    case "statefulsets":
    case "daemonsets":
    case "jobs":
    case "replicasets":
      return ["spec", "template", "spec"];
    default:
      return null;
  }
}

type Json = Record<string, unknown>;
const obj = (v: unknown): Json => (v && typeof v === "object" && !Array.isArray(v) ? (v as Json) : {});
const str = (v: unknown) => (typeof v === "string" ? v : v == null ? "" : String(v));

/** Equal as JSON, whatever the key order. */
export function sameSpec(a: unknown, b: unknown): boolean {
  const canon = (v: unknown): unknown =>
    Array.isArray(v)
      ? v.map(canon)
      : v && typeof v === "object"
        ? Object.fromEntries(Object.keys(v as Json).sort().map((k) => [k, canon((v as Json)[k])]))
        : v;
  return JSON.stringify(canon(a ?? null)) === JSON.stringify(canon(b ?? null));
}

/** Where an env var's value comes from. */
export function envSource(spec: EnvSpec): EnvSource {
  if (!spec.valueFrom) return { kind: "value", value: str(spec.value) };
  const from = obj(spec.valueFrom);
  const cm = obj(from.configMapKeyRef);
  const sec = obj(from.secretKeyRef);
  if (from.configMapKeyRef) return { kind: "configmaps", name: str(cm.name), key: str(cm.key) };
  if (from.secretKeyRef) return { kind: "secrets", name: str(sec.name), key: str(sec.key) };
  if (from.fieldRef) return { kind: "field", path: str(obj(from.fieldRef).fieldPath) };
  if (from.resourceFieldRef) return { kind: "resource", resource: str(obj(from.resourceFieldRef).resource) };
  return { kind: "other" };
}

const specOf = (e: Json): EnvSpec => {
  const rest = { ...e };
  delete rest.name;
  return rest as EnvSpec;
};

function containersOf(list: unknown, init: boolean): Container[] {
  if (!Array.isArray(list)) return [];
  return list.map((c: Json) => ({
    name: str(c.name),
    init,
    image: str(c.image),
    env: (Array.isArray(c.env) ? (c.env as Json[]) : []).map((e) => {
      const spec = specOf(e);
      return { name: str(e.name), spec, source: envSource(spec) };
    }),
    envFrom: (Array.isArray(c.envFrom) ? (c.envFrom as Json[]) : []).flatMap((f): EnvFrom[] => {
      const prefix = f.prefix ? str(f.prefix) : undefined;
      if (f.configMapRef) return [{ kind: "configmaps", name: str(obj(f.configMapRef).name), prefix }];
      if (f.secretRef) return [{ kind: "secrets", name: str(obj(f.secretRef).name), prefix }];
      return [];
    }),
  }));
}

function readTweaks(raw: unknown): Tweaks | null {
  if (typeof raw !== "string") return null;
  try {
    const t = JSON.parse(raw) as Tweaks;
    return t && typeof t === "object" ? t : null;
  } catch {
    return null;
  }
}

/** What the object page shows from an object's YAML. */
export function readSpec(kind: KubeKind, text: string): ObjectSpec {
  let o: Json = {};
  try {
    o = obj(parse(text));
  } catch {
    // A cut or unreadable document: the page still has its summary and YAML tab.
  }
  const meta = obj(o.metadata);
  const labels = Object.entries(obj(meta.labels)).map(([k, v]): [string, string] => [k, str(v)]);
  const ann = obj(meta.annotations);
  const annotations = Object.entries(ann)
    .filter(([k]) => k !== LAST_APPLIED && k !== TWEAKS)
    .map(([k, v]): [string, string] => [k, str(v)]);
  const path = podSpecPath(kind);
  let containers: Container[] | null = null;
  if (path) {
    const pod = obj(path.reduce<unknown>((v, k) => obj(v)[k], o));
    containers = [...containersOf(pod.initContainers, true), ...containersOf(pod.containers, false)];
  }
  let data: DataEntry[] | null = null;
  if (kind === "configmaps" || kind === "secrets") {
    const secret = kind === "secrets";
    data = [
      ...Object.entries(obj(o.data)).map(([key, v]) => ({ key, value: secret && v === SECRET_MASK ? null : str(v) })),
      ...Object.entries(obj(o.binaryData)).map(([key, v]) => ({ key, value: null, bytes: Math.floor((str(v).length * 3) / 4) })),
    ];
  }
  return {
    labels,
    annotations,
    containers,
    data,
    immutable: o.immutable === true,
    managedBy: str(obj(meta.labels)["app.kubernetes.io/managed-by"]) || undefined,
    tweaks: kind === "secrets" ? null : readTweaks(ann[TWEAKS]),
  };
}

/** Env vars and keys a tweak changed that still differ from what they replaced. */
export function activeTweaks(spec: ObjectSpec): Op[] {
  const t = spec.tweaks;
  if (!t) return [];
  const out: Op[] = [];
  for (const [container, vars] of Object.entries(t.env ?? {})) {
    const c = spec.containers?.find((x) => x.name === container);
    if (!c) continue;
    for (const [name, was] of Object.entries(vars)) {
      const now = c.env.findLast((e) => e.name === name)?.spec ?? null;
      if (!sameSpec(now, was)) out.push({ op: "env", container, name, to: was });
    }
  }
  for (const [key, was] of Object.entries(t.data ?? {})) {
    const now = spec.data?.find((d) => d.key === key)?.value ?? null;
    if (now !== was) out.push({ op: "data", key, to: was });
  }
  return out;
}

/** The ops that put back what a tweak replaced: all of them, or just one. */
export const revertOps = (spec: ObjectSpec, only?: (op: Op) => boolean): Op[] => activeTweaks(spec).filter((o) => !only || only(o));

function containerNode(doc: Document, kind: KubeKind, name: string): YAMLMap | undefined {
  const path = podSpecPath(kind);
  if (!path) return undefined;
  for (const field of ["containers", "initContainers"]) {
    const seq = doc.getIn([...path, field]);
    if (!isSeq(seq)) continue;
    const hit = seq.items.find((c) => isMap(c) && c.get("name") === name);
    if (isMap(hit)) return hit;
  }
  return undefined;
}

/**
 * A string as a quoted scalar (a block for several lines): the apiserver
 * reads YAML 1.1, where a bare `on`, `yes` or `0644` is not a string.
 */
function quoted(v: string): Scalar {
  const s = new Scalar(v);
  s.type = v.includes("\n") ? Scalar.BLOCK_LITERAL : Scalar.QUOTE_DOUBLE;
  return s;
}

/** An env entry, its literal value quoted. */
function envNode(doc: Document, name: string, to: EnvSpec): YAMLMap {
  const m = new YAMLMap();
  m.set("name", quoted(name));
  if (to.value !== undefined) m.set("value", quoted(to.value));
  if (to.valueFrom !== undefined) m.set("valueFrom", doc.createNode(to.valueFrom));
  return m;
}

const envOf = (c: YAMLMap): YAMLSeq => {
  const env = c.get("env", true);
  if (isSeq(env)) return env;
  const seq = new YAMLSeq();
  c.set("env", seq);
  return seq;
};

/**
 * Applies `ops` to the object as the editor opened it. Outside Secrets, the
 * values they replace are kept in the `fleetops.io/tweaks` annotation (the
 * first value only, so a second tweak still reverts to the original), and an
 * entry is dropped once its value is back where it started.
 */
export function applyOps(kind: KubeKind, text: string, ops: Op[], who?: string, now = Date.now()): string {
  const doc = parseDocument(text);
  const secret = kind === "secrets";
  const record: Tweaks | null = secret ? null : (readTweaks(doc.getIn(["metadata", "annotations", TWEAKS])) ?? { at: 0 });
  for (const op of ops) {
    if (op.op === "env") {
      const c = containerNode(doc, kind, op.container);
      if (!c) throw new Error(`container ${op.container} is no longer in this ${kind.replace(/s$/, "")}`);
      const env = envOf(c);
      const idx = env.items.findLastIndex((e) => isMap(e) && e.get("name") === op.name);
      const was = idx >= 0 ? specOf(obj((env.items[idx] as YAMLMap).toJSON())) : null;
      if (record) {
        const byC = ((record.env ??= {})[op.container] ??= {});
        if (!(op.name in byC)) byC[op.name] = was;
      }
      if (op.to === null) {
        if (idx >= 0) env.items.splice(idx, 1);
        if (env.items.length === 0) c.delete("env");
      } else if (idx >= 0) {
        env.items[idx] = envNode(doc, op.name, op.to);
      } else {
        env.items.push(envNode(doc, op.name, op.to));
      }
    } else {
      const was = doc.getIn(["data", op.key]);
      if (record) {
        const data = (record.data ??= {});
        if (!(op.key in data)) data[op.key] = was == null ? null : str(was);
      }
      if (secret) {
        // A Secret's `data` is base64; a new value goes in `stringData`, which the apiserver encodes.
        doc.deleteIn(["data", op.key]);
        if (op.to === null) doc.deleteIn(["stringData", op.key]);
        else doc.setIn(["stringData", op.key], quoted(op.to));
      } else if (op.to === null) {
        doc.deleteIn(["data", op.key]);
      } else {
        doc.setIn(["data", op.key], quoted(op.to));
      }
    }
  }
  if (record) {
    const after = readSpec(kind, doc.toString());
    for (const [container, vars] of Object.entries(record.env ?? {})) {
      const c = after.containers?.find((x) => x.name === container);
      for (const [name, was] of Object.entries(vars)) {
        if (!c || sameSpec(c.env.findLast((e) => e.name === name)?.spec ?? null, was)) delete vars[name];
      }
      if (Object.keys(vars).length === 0) delete record.env![container];
    }
    for (const [key, was] of Object.entries(record.data ?? {})) {
      if ((after.data?.find((d) => d.key === key)?.value ?? null) === was) delete record.data![key];
    }
    if (record.env && Object.keys(record.env).length === 0) delete record.env;
    if (record.data && Object.keys(record.data).length === 0) delete record.data;
    if (!record.env && !record.data) {
      doc.deleteIn(["metadata", "annotations", TWEAKS]);
      const ann = doc.getIn(["metadata", "annotations"]);
      if (isMap(ann) && ann.items.length === 0) doc.deleteIn(["metadata", "annotations"]);
    } else {
      const { env, data } = record;
      doc.setIn(["metadata", "annotations", TWEAKS], JSON.stringify({ by: who, at: Math.floor(now / 1000), env, data }));
    }
  }
  return doc.toString();
}

/** One op in words, for the review: the verb, the variable or key, its container, and the values either side. */
export function describeOp(op: Op, spec: ObjectSpec): { verb: "Add" | "Change" | "Remove"; subject: string; where?: string; before?: string; after?: string } {
  if (op.op === "data") {
    const had = spec.data?.find((d) => d.key === op.key);
    const before = had?.value ?? undefined;
    if (op.to === null) return { verb: "Remove", subject: op.key, before };
    if (!had) return { verb: "Add", subject: op.key, after: op.to };
    return { verb: "Change", subject: op.key, before, after: op.to };
  }
  const c = spec.containers?.find((x) => x.name === op.container);
  const had = c?.env.findLast((e) => e.name === op.name);
  const where = (spec.containers?.length ?? 0) > 1 ? op.container : undefined;
  const before = had && sourceText(had.source);
  if (op.to === null) return { verb: "Remove", subject: op.name, where, before };
  const after = sourceText(envSource(op.to));
  if (!had) return { verb: "Add", subject: op.name, where, after };
  return { verb: "Change", subject: op.name, where, before, after };
}

/** A non-literal source in words: "Secret api-secrets, key DATABASE_URL". */
export function sourceText(s: EnvSource): string {
  switch (s.kind) {
    case "value":
      return s.value;
    case "configmaps":
      return `ConfigMap ${s.name}, key ${s.key}`;
    case "secrets":
      return `Secret ${s.name}, key ${s.key}`;
    case "field":
      return `field ${s.path}`;
    case "resource":
      return `resource ${s.resource}`;
    default:
      return "another source";
  }
}

/** Valid as an env var name: what the apiserver accepts (C identifiers, plus `-` and `.`). */
export const validEnvName = (n: string) => /^[-._a-zA-Z][-._a-zA-Z0-9]*$/.test(n);
/** Valid as a ConfigMap or Secret key. */
export const validDataKey = (k: string) => /^[-._a-zA-Z0-9]+$/.test(k) && k.length <= 253;
