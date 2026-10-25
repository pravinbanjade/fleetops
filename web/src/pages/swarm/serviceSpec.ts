/** The fields the service form edits; everything else in the spec is kept. */
export interface ServiceForm {
  name: string;
  image: string;
  mode: "replicated" | "global";
  replicas: number;
  /** `KEY=value` per line. */
  env: string;
  /** `published:target` or `published:target/udp` per line. */
  ports: string;
  /** Placement constraints, one per line (`node.role == manager`). */
  constraints: string;
}

// Docker's ServiceSpec, loosely: the form reads and writes a few paths.
// eslint-disable-next-line @typescript-eslint/no-explicit-any
type Spec = Record<string, any>;

interface Port {
  Protocol?: string;
  TargetPort?: number;
  PublishedPort?: number;
}

export const EMPTY_FORM: ServiceForm = { name: "", image: "", mode: "replicated", replicas: 1, env: "", ports: "", constraints: "" };

const lines = (text: string) =>
  text
    .split("\n")
    .map((l) => l.trim())
    .filter(Boolean);

export function formFromSpec(spec: Spec): ServiceForm {
  const cs = spec.TaskTemplate?.ContainerSpec ?? {};
  const ports: Port[] = spec.EndpointSpec?.Ports ?? [];
  const global = spec.Mode?.Global !== undefined;
  return {
    name: spec.Name ?? "",
    image: cs.Image ?? "",
    mode: global ? "global" : "replicated",
    replicas: spec.Mode?.Replicated?.Replicas ?? 1,
    env: (cs.Env ?? []).join("\n"),
    ports: ports.map((p) => `${p.PublishedPort ?? 0}:${p.TargetPort ?? 0}${p.Protocol && p.Protocol !== "tcp" ? `/${p.Protocol}` : ""}`).join("\n"),
    constraints: (spec.TaskTemplate?.Placement?.Constraints ?? []).join("\n"),
  };
}

function parsePorts(text: string): Port[] {
  return lines(text).map((l, i) => {
    const m = /^(\d+):(\d+)(?:\/(tcp|udp|sctp))?$/.exec(l);
    if (!m) throw new Error(`line ${i + 1}: use published:target or published:target/udp`);
    return { Protocol: m[3] ?? "tcp", TargetPort: Number(m[2]), PublishedPort: Number(m[1]) };
  });
}

/** `base` (deep-copied) with the form's fields written in; throws on a bad port line. */
export function specFromForm(form: ServiceForm, base: Spec = {}): Spec {
  const spec: Spec = structuredClone(base);
  spec.Name = form.name.trim();
  spec.TaskTemplate ??= {};
  spec.TaskTemplate.ContainerSpec ??= {};
  const cs = spec.TaskTemplate.ContainerSpec;
  cs.Image = form.image.trim();
  const env = lines(form.env);
  if (env.length) cs.Env = env;
  else delete cs.Env;
  spec.Mode = form.mode === "global" ? { Global: {} } : { Replicated: { Replicas: form.replicas } };
  const ports = parsePorts(form.ports);
  if (ports.length) spec.EndpointSpec = { ...spec.EndpointSpec, Ports: ports };
  else if (spec.EndpointSpec) delete spec.EndpointSpec.Ports;
  const constraints = lines(form.constraints);
  if (constraints.length) spec.TaskTemplate.Placement = { ...spec.TaskTemplate.Placement, Constraints: constraints };
  else if (spec.TaskTemplate.Placement) delete spec.TaskTemplate.Placement.Constraints;
  return spec;
}
