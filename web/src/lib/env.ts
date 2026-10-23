/** Hues an environment's identity tag can take: none of them a state colour. */
const HUES = [255, 292, 328, 205, 232, 270];

/** A stable hue for an environment, so prod and staging never look alike. */
export function envHue(name: string): number {
  let h = 0;
  for (const ch of name) h = (h * 31 + ch.charCodeAt(0)) >>> 0;
  return HUES[h % HUES.length];
}

/** Up to two letters naming an environment on its tag. */
export function envInitials(name: string): string {
  const parts = name.split(/[^a-zA-Z0-9]+/).filter(Boolean);
  const s = parts.length > 1 ? parts[0][0] + parts[1][0] : name.slice(0, 2);
  return s.toUpperCase();
}

/** Kubernetes environment kinds start with "kube-". */
export function isKube(kind: string | undefined): boolean {
  return !!kind && kind.startsWith("kube-");
}

/** Swarm environment kinds start with "swarm-". */
export function isSwarm(kind: string | undefined): boolean {
  return !!kind && kind.startsWith("swarm-");
}

/** Which set of pages an environment has; a swarm node's view has the Docker pages, less stacks. */
export type EnvFamily = "docker" | "kube" | "swarm" | "node";

export function envFamily(kind: string | undefined): EnvFamily {
  return kind === "swarm-node" ? "node" : isKube(kind) ? "kube" : isSwarm(kind) ? "swarm" : "docker";
}

/** `<swarm>@<node>`: a Swarm node's view. */
export function splitNodeView(name: string | undefined): { swarm: string; node: string } | null {
  const at = name?.indexOf("@") ?? -1;
  if (!name || at <= 0 || at === name.length - 1) return null;
  return { swarm: name.slice(0, at), node: name.slice(at + 1) };
}

/**
 * The environment `name` names: listed ones as listed, and a swarm node's
 * view as its swarm (access included) under its own name and kind.
 */
export function findEnv<T extends { name: string; kind: string }>(envs: readonly T[] | undefined, name: string | undefined): T | undefined {
  const direct = envs?.find((e) => e.name === name);
  if (direct) return direct;
  const view = splitNodeView(name);
  const swarm = view && envs?.find((e) => e.name === view.swarm && isSwarm(e.kind));
  return swarm ? { ...swarm, name: name!, kind: "swarm-node" } : undefined;
}
