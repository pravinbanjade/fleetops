import type { ContainerSpec } from "@/api/gen/types.gen";
import { joinArgs, splitArgs } from "./args";
import { formatKeyValues, parseKeyValues } from "./kv";

export const RESTART_POLICIES = ["no", "always", "unless-stopped", "on-failure"] as const;

export interface PortRow {
  host: string;
  container: string;
  protocol: "tcp" | "udp";
}

/** The container form's fields, as the inputs hold them (text, mostly). */
export interface SpecForm {
  image: string;
  name: string;
  cmd: string;
  entrypoint: string;
  ports: PortRow[];
  /** One `KEY=value` per line. */
  env: string;
  /** One `source:target[:mode]` per line. */
  binds: string;
  network: string;
  restart: (typeof RESTART_POLICIES)[number];
  /** One `key=value` per line. */
  labels: string;
  memoryMb: string;
  cpus: string;
  /** Capabilities separated by spaces or commas. */
  capAdd: string;
  privileged: boolean;
  /** A Docker `ContainerCreate` JSON object merged over the rest. */
  advanced: string;
}

export const EMPTY_FORM: SpecForm = {
  image: "",
  name: "",
  cmd: "",
  entrypoint: "",
  ports: [],
  env: "",
  binds: "",
  network: "",
  restart: "no",
  labels: "",
  memoryMb: "",
  cpus: "",
  capAdd: "",
  privileged: false,
  advanced: "",
};

const MB = 1024 * 1024;

/** The form pre-filled from an existing container's spec (recreate, duplicate). */
export function specToForm(s: ContainerSpec): SpecForm {
  const policy = RESTART_POLICIES.find((p) => p === s.restart_policy) ?? "no";
  return {
    image: s.image ?? "",
    name: s.name ?? "",
    cmd: s.cmd ? joinArgs(s.cmd) : "",
    entrypoint: s.entrypoint ? joinArgs(s.entrypoint) : "",
    ports: (s.ports ?? []).map((p) => ({
      host: p.host_port == null ? "" : String(p.host_port),
      container: String(p.container_port),
      protocol: p.protocol === "udp" ? "udp" : "tcp",
    })),
    env: (s.env ?? []).join("\n"),
    binds: (s.binds ?? []).join("\n"),
    network: s.network ?? "",
    restart: policy,
    labels: formatKeyValues(s.labels),
    memoryMb: s.memory_bytes ? String(Math.round(s.memory_bytes / MB)) : "",
    cpus: s.nano_cpus ? String(s.nano_cpus / 1e9) : "",
    capAdd: (s.cap_add ?? []).join(" "),
    privileged: !!s.privileged,
    advanced: s.advanced ? JSON.stringify(s.advanced, null, 2) : "",
  };
}

const lines = (text: string) =>
  text
    .split("\n")
    .map((l) => l.trim())
    .filter(Boolean);

function port(text: string, what: string): number {
  const n = Number(text);
  if (!Number.isInteger(n) || n < 1 || n > 65535) throw new Error(`${what} must be a number from 1 to 65535, not "${text}"`);
  return n;
}

function positive(text: string, what: string): number {
  const n = Number(text);
  if (!Number.isFinite(n) || n <= 0) throw new Error(`${what} must be a positive number, not "${text}"`);
  return n;
}

/** The spec to send, or throws an Error saying which field is wrong. */
export function formToSpec(f: SpecForm): ContainerSpec {
  if (!f.image.trim()) throw new Error("Image is required");
  const labels = parseKeyValues(f.labels, "Label");
  for (const e of lines(f.env)) {
    if (e.startsWith("=")) throw new Error(`Environment variable "${e}" has no name`);
  }
  let advanced: Record<string, unknown> | null = null;
  if (f.advanced.trim()) {
    let parsed: unknown;
    try {
      parsed = JSON.parse(f.advanced);
    } catch (e) {
      throw new Error(`Advanced settings are not valid JSON: ${(e as Error).message}`, { cause: e });
    }
    if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
      throw new Error("Advanced settings must be a JSON object");
    }
    advanced = parsed as Record<string, unknown>;
  }
  const args = (text: string, what: string) => {
    try {
      return text.trim() ? splitArgs(text) : null;
    } catch (e) {
      throw new Error(`${what}: ${(e as Error).message}`, { cause: e });
    }
  };
  return {
    image: f.image.trim(),
    name: f.name.trim() || null,
    cmd: args(f.cmd, "Command"),
    entrypoint: args(f.entrypoint, "Entrypoint"),
    ports: f.ports
      .filter((p) => p.container.trim() || p.host.trim())
      .map((p) => ({
        container_port: port(p.container.trim(), "Container port"),
        host_port: p.host.trim() ? port(p.host.trim(), "Host port") : null,
        protocol: p.protocol,
      })),
    env: lines(f.env),
    binds: lines(f.binds),
    network: f.network || null,
    restart_policy: f.restart,
    labels,
    memory_bytes: f.memoryMb.trim() ? Math.round(positive(f.memoryMb.trim(), "Memory") * MB) : null,
    nano_cpus: f.cpus.trim() ? Math.round(positive(f.cpus.trim(), "CPUs") * 1e9) : null,
    cap_add: f.capAdd.split(/[\s,]+/).filter(Boolean),
    privileged: f.privileged,
    advanced,
  };
}

