import type { ContainerSummary } from "@/api/gen/types.gen";

/** The containers mounting each named volume, from the container list. */
export function volumeUsers(containers: ContainerSummary[] | undefined): Map<string, ContainerSummary[]> {
  const out = new Map<string, ContainerSummary[]>();
  for (const c of containers ?? []) {
    for (const v of c.volumes) out.set(v, [...(out.get(v) ?? []), c]);
  }
  return out;
}
