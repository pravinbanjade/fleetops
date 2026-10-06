import { diffLines } from "diff";

/** A unified line diff; added and removed lines are <ins>/<del> for screen readers. */
export function DiffView({ before, after, label }: { before?: string | null; after?: string | null; label: string }) {
  const parts = diffLines(before ?? "", after ?? "");
  return (
    <pre aria-label={label} className="max-h-[50vh] overflow-auto rounded-md border bg-muted/40 p-3 font-mono text-xs">
      {parts.flatMap((p, i) =>
        p.value
          .replace(/\n$/, "")
          .split("\n")
          .map((line, j) => {
            const k = `${i}-${j}`;
            if (p.added) return <ins key={k} className="block bg-state-ok/15 no-underline">+ {line}</ins>;
            if (p.removed) return <del key={k} className="block bg-state-fail/15 no-underline">- {line}</del>;
            return <span key={k} className="block text-muted-foreground">  {line}</span>;
          }),
      )}
    </pre>
  );
}
