/** An image reference, set like a container name; a digest is cut to its first 12 hex digits. */
export function ImageRef({ value }: { value: string }) {
  return (
    <span className="font-ident font-medium" title={value}>
      {value.replace(/@(sha256:[0-9a-f]{12})[0-9a-f]+$/, "@$1…")}
    </span>
  );
}
