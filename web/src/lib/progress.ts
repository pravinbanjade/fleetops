export interface LayerProgress {
  id: string;
  status: string;
  current?: number;
  total?: number;
}

interface DockerProgress {
  id?: string;
  status?: string;
  progressDetail?: { current?: number; total?: number };
  stream?: string;
  error?: string;
  aux?: { ID?: string };
}

const LAYER = /^[0-9a-f]{12}$/;

/**
 * An image pull's progress messages as one row per layer (its latest
 * status and byte counts) plus the other status lines in order.
 */
export function pullProgress(messages: unknown[]): { layers: LayerProgress[]; lines: string[] } {
  const layers = new Map<string, LayerProgress>();
  const lines: string[] = [];
  for (const raw of messages) {
    const m = raw as DockerProgress;
    if (m.error) lines.push(m.error);
    else if (m.id && LAYER.test(m.id) && m.status) {
      const d = m.progressDetail;
      const old = layers.get(m.id);
      layers.set(m.id, {
        id: m.id,
        status: m.status,
        current: d?.total ? d.current : m.status.endsWith("complete") ? old?.total : undefined,
        total: d?.total || (m.status.endsWith("complete") ? old?.total : undefined),
      });
    } else if (m.status) lines.push(m.id ? `${m.status} ${m.id}`.trim() : m.status);
  }
  return { layers: [...layers.values()], lines };
}

/** A build's progress messages as log lines, without Docker's blank separator lines. */
export function buildLog(messages: unknown[]): string[] {
  const out: string[] = [];
  for (const raw of messages) {
    const m = raw as DockerProgress;
    if (m.stream) out.push(...m.stream.split("\n").filter((l) => l.trim()));
    else if (m.error) out.push(m.error);
    else if (m.aux?.ID) out.push(`Built ${m.aux.ID}`);
    else if (m.status) out.push(m.id ? `${m.id}: ${m.status}` : m.status);
  }
  return out;
}
