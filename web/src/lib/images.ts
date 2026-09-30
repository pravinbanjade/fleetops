/**
 * A `repo[:tag]` reference split into its parts; the tag defaults to
 * `latest`. A colon before the last slash belongs to a registry port
 * (`registry:5000/app` has no tag).
 */
export function parseRef(ref: string): { repo: string; tag: string } {
  const r = ref.trim();
  const slash = r.lastIndexOf("/");
  const colon = r.lastIndexOf(":");
  return colon > slash ? { repo: r.slice(0, colon), tag: r.slice(colon + 1) } : { repo: r, tag: "latest" };
}
