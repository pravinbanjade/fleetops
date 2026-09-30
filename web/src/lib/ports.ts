import type { PortMapping } from "@/api/gen/types.gen";

export interface PortLink {
  label: string;
  /** Set for published TCP ports: where a browser can reach the port. */
  href?: string;
}

const ANY = ["0.0.0.0", "::", ""];

/**
 * A container's ports as the list shows them: one per published port
 * (Docker reports IPv4 and IPv6 bindings of the same port separately), or,
 * when nothing is published, the exposed ports. A port bound to all
 * addresses links to `hostname`, the host the UI was loaded from.
 */
export function portLinks(ports: PortMapping[], hostname: string): PortLink[] {
  const published = new Map<string, PortLink>();
  const exposed = new Map<string, PortLink>();
  for (const p of ports) {
    if (p.public_port == null) {
      const label = `${p.private_port}/${p.protocol}`;
      exposed.set(label, { label });
      continue;
    }
    const any = ANY.includes(p.ip ?? "");
    const host = any ? hostname : p.ip!;
    const key = `${any ? "*" : host}:${p.public_port}/${p.protocol}`;
    const label = `${p.public_port}→${p.private_port}${p.protocol === "tcp" ? "" : `/${p.protocol}`}`;
    const urlHost = host.includes(":") ? `[${host}]` : host;
    published.set(key, p.protocol === "tcp" ? { label, href: `http://${urlHost}:${p.public_port}` } : { label });
  }
  return [...(published.size ? published : exposed).values()];
}
