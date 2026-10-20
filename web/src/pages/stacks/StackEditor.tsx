import { lazy, Suspense, useEffect, useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { saveStackDraft, validateStack } from "@/api/gen/sdk.gen";
import type { ComposeIssue, StackDetail } from "@/api/gen/types.gen";
import { keys } from "@/api/queries";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { errorMessage } from "@/lib/errors";
import { FormError } from "@/pages/AuthLayout";

const CodeMirrorField = lazy(() => import("@/components/yaml-editor/CodeMirrorField"));

/** How long the editor waits after typing before validating. */
export const VALIDATE_DEBOUNCE_MS = 500;

type Row = { key: string; value: string };

const toRows = (vars: string[]): Row[] =>
  vars.map((v) => {
    const i = v.indexOf("=");
    return i < 0 ? { key: v, value: "" } : { key: v.slice(0, i), value: v.slice(i + 1) };
  });
const toLines = (rows: Row[]) => rows.filter((r) => r.key.trim()).map((r) => `${r.key.trim()}=${r.value}`);

/** The Compose file and variables of a stack's draft, validated as you type. */
export function StackEditor({
  env,
  detail,
  canEdit,
  canDeploy,
  onReview,
}: {
  env: string;
  detail: StackDetail;
  canEdit: boolean;
  canDeploy: boolean;
  onReview: () => void;
}) {
  const qc = useQueryClient();
  const name = detail.stack.name;
  const [compose, setCompose] = useState(detail.draft.compose);
  const [rows, setRows] = useState<Row[]>(() => toRows(detail.draft.variables));
  const [issues, setIssues] = useState<ComposeIssue[]>([]);
  const [warnings, setWarnings] = useState<ComposeIssue[]>([]);
  const lines = toLines(rows);
  const linesKey = lines.join("\n");
  useEffect(() => {
    const t = setTimeout(async () => {
      try {
        const { data } = await validateStack({
          path: { env },
          body: { name, compose, variables: linesKey ? linesKey.split("\n") : [] },
        });
        setIssues(data.issues);
        setWarnings(data.warnings);
      } catch {
        // The review shows any problem the server finds; validation here is a hint.
      }
    }, VALIDATE_DEBOUNCE_MS);
    return () => clearTimeout(t);
  }, [env, name, compose, linesKey]);
  const save = useMutation({
    meta: { inline: true },
    mutationFn: async () => {
      await saveStackDraft({ path: { env, stack: name }, body: { compose, variables: lines } });
      await qc.invalidateQueries({ queryKey: keys.stack(env, name) });
      await qc.invalidateQueries({ queryKey: keys.stacks(env) });
    },
  });
  const review = async () => {
    if (canEdit) await save.mutateAsync();
    onReview();
  };
  const setRow = (i: number, r: Partial<Row>) => setRows((old) => old.map((o, n) => (n === i ? { ...o, ...r } : o)));
  return (
    <div className="grid gap-4">
      {canEdit ? (
        <Suspense fallback={<p className="text-sm text-muted-foreground">Loading editor…</p>}>
          <CodeMirrorField label="Compose file" value={compose} onChange={setCompose} />
        </Suspense>
      ) : (
        <pre aria-label="Compose file" className="overflow-auto rounded-md border bg-muted/40 p-3 font-mono text-xs">
          {compose}
        </pre>
      )}
      {issues.length > 0 && (
        <ul aria-label="Problems" className="grid gap-0.5 text-sm text-destructive">
          {issues.map((i, n) => (
            <li key={n}>
              <span className="font-ident">{i.path || "file"}</span>: {i.message}
            </li>
          ))}
        </ul>
      )}
      {warnings.length > 0 && (
        <ul aria-label="Warnings" className="grid gap-0.5 text-sm text-muted-foreground">
          {warnings.map((w, n) => (
            <li key={n}>
              <span className="font-ident">{w.path}</span>: {w.message}
            </li>
          ))}
        </ul>
      )}
      <section aria-label="Variables" className="grid gap-2">
        <div className="grid gap-0.5">
          <h2 className="text-base font-semibold">Variables</h2>
          <p className="text-sm text-muted-foreground">
            Used as <span className="font-ident">${"{NAME}"}</span> in the file. Stored encrypted; values shown masked
            stay as they are when saved.
          </p>
        </div>
        {rows.map((r, i) => (
          <div key={i} className="grid grid-cols-[minmax(0,12rem)_minmax(0,1fr)_auto] gap-2">
            <Input
              aria-label={`Variable ${i + 1} name`}
              className="font-ident"
              value={r.key}
              disabled={!canEdit}
              onChange={(e) => setRow(i, { key: e.target.value })}
            />
            <Input
              aria-label={`Variable ${i + 1} value`}
              value={r.value}
              disabled={!canEdit}
              onChange={(e) => setRow(i, { value: e.target.value })}
            />
            {canEdit && (
              <Button variant="outline" size="sm" onClick={() => setRows((old) => old.filter((_, n) => n !== i))}>
                Remove
              </Button>
            )}
          </div>
        ))}
        {canEdit && (
          <div>
            <Button variant="outline" size="sm" onClick={() => setRows((old) => [...old, { key: "", value: "" }])}>
              Add variable
            </Button>
          </div>
        )}
      </section>
      <FormError message={save.error ? errorMessage(save.error) : null} />
      <div className="flex flex-wrap gap-2">
        {canEdit && (
          <Button variant="outline" disabled={save.isPending} onClick={() => save.mutate()}>
            Save draft
          </Button>
        )}
        {canDeploy && (
          <Button disabled={save.isPending} onClick={() => void review()}>
            Review deploy
          </Button>
        )}
      </div>
    </div>
  );
}
