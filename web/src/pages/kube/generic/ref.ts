import type { ApiKind, Discovery } from "@/api/gen/types.gen";

/** A discovered kind as URLs name it: the core group is `core`. */
export type Ref = { group: string; version: string; resource: string };

export const refOf = (k: ApiKind): Ref => ({ group: k.api.group || "core", version: k.api.version, resource: k.api.resource });

/** The kind string a discovered kind's watch and cached list go by. */
export const refKind = (r: Ref) => `r/${r.group}/${r.version}/${r.resource}`;

export const findKind = (d: Discovery | undefined, r: Ref) =>
  d?.kinds.find((k) => (k.api.group || "core") === r.group && k.api.version === r.version && k.api.resource === r.resource);
