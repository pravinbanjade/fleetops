import type { EntryKind } from "@/api/gen/types.gen";

const TYPE: Record<EntryKind, string> = { dir: "d", symlink: "l", file: "-", other: "?" };

/** Permission bits as `ls -l` shows them: `drwxr-xr-x`. */
export function formatMode(mode: number, kind: EntryKind): string {
  let out = TYPE[kind];
  for (const shift of [6, 3, 0]) {
    const bits = (mode >> shift) & 7;
    out += (bits & 4 ? "r" : "-") + (bits & 2 ? "w" : "-") + (bits & 1 ? "x" : "-");
  }
  return out;
}

/** `dir` joined with `name`: `/` + `etc` is `/etc`, `/etc` + `hosts` is `/etc/hosts`. */
export function joinPath(dir: string, name: string): string {
  return `${dir.replace(/\/+$/, "")}/${name}`;
}

/** Each ancestor of `path` with its name, for breadcrumbs: `/etc/nginx` → `/`, `/etc`, `/etc/nginx`. */
export function crumbs(path: string): { name: string; path: string }[] {
  const parts = path.split("/").filter(Boolean);
  return [{ name: "/", path: "/" }, ...parts.map((name, i) => ({ name, path: "/" + parts.slice(0, i + 1).join("/") }))];
}
