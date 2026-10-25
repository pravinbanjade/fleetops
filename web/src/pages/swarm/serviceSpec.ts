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

/**
 * `base` (deep-copied) with the form's changes written in: a field the user
 * left as it was is not rewritten, so what the form cannot show (port publish
 * modes, job modes, spacing in values) survives. Throws on a bad port line.
 */
export function specFromForm(form: ServiceForm, base: Spec = {}): Spec {
  const spec: Spec = structuredClone(base);
  // A new spec gets every field; an edit only the ones that changed.
  const was: ServiceForm | null = Object.keys(base).length ? formFromSpec(base) : null;
  const changed = (...k: (keyof ServiceForm)[]) => !was || k.some((key) => form[key] !== was[key]);
  spec.TaskTemplate ??= {};
  spec.TaskTemplate.ContainerSpec ??= {};
  const cs = spec.TaskTemplate.ContainerSpec;
  if (changed("name")) spec.Name = form.name.trim();
  if (changed("image")) cs.Image = form.image.trim();
  if (changed("env")) {
    const env = lines(form.env);
    if (env.length) cs.Env = env;
    else delete cs.Env;
  }
  if (changed("mode", "replicas")) spec.Mode = form.mode === "global" ? { Global: {} } : { Replicated: { Replicas: form.replicas } };
  if (changed("ports")) {
    const old: Port[] = base.EndpointSpec?.Ports ?? [];
    // Keep what the form does not show (publish mode, name) on ports it still lists.
    const ports = parsePorts(form.ports).map((p) => ({ ...old.find((o) => o.TargetPort === p.TargetPort && (o.Protocol ?? "tcp") === p.Protocol), ...p }));
    if (ports.length) spec.EndpointSpec = { ...spec.EndpointSpec, Ports: ports };
    else if (spec.EndpointSpec) delete spec.EndpointSpec.Ports;
  }
  if (changed("constraints")) {
    const constraints = lines(form.constraints);
    if (constraints.length) spec.TaskTemplate.Placement = { ...spec.TaskTemplate.Placement, Constraints: constraints };
    else if (spec.TaskTemplate.Placement) delete spec.TaskTemplate.Placement.Constraints;
  }
  return spec;
}
