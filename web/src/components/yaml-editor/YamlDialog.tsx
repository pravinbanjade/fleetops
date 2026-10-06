import { useQueryClient } from "@tanstack/react-query";
import { lazy, Suspense, useEffect, useState } from "react";
import { toast } from "sonner";
import { ApiError } from "@/api/client";
import { applyKube, editableKube, kubeTemplates, replaceKube } from "@/api/gen/sdk.gen";
import type { ApplyResult, KubeKind } from "@/api/gen/types.gen";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogTitle } from "@/components/ui/dialog";
import { errorMessage } from "@/lib/errors";
import { KIND_LABEL } from "@/pages/kube/kinds";
import { yamlErrors } from "./check";
import { DiffView } from "./DiffView";

const CodeMirrorField = lazy(() => import("./CodeMirrorField"));

export type YamlTarget =
  | { mode: "edit"; kind: KubeKind; ns: string; name: string }
  | { mode: "create"; kind: KubeKind; namespace: string }
  | { mode: "apply"; namespace: string };

type Stage = "edit" | "review" | "failed";

const title = (t: YamlTarget) =>
  t.mode === "edit" ? `Edit ${t.name}` : t.mode === "create" ? `Create ${KIND_LABEL[t.kind].replace(/s$/, "")}` : "Apply YAML";

/**
 * Edit, create or apply Kubernetes YAML (spec §4.2–4.3): Review sends a dry
 * run and shows what would change; nothing is written until Apply.
 */
export default function YamlDialog({ env, target, onClose }: { env: string; target: YamlTarget; onClose: () => void }) {
  const qc = useQueryClient();
  const [text, setText] = useState("");
  const [loaded, setLoaded] = useState("");
  const [rv, setRv] = useState("");
  const [stage, setStage] = useState<Stage>("edit");
  const [results, setResults] = useState<ApplyResult[]>([]);
  const [problems, setProblems] = useState<string[]>([]);
  const [conflict, setConflict] = useState<{ yaml: string; rv: string } | null>(null);
  const [confirmForce, setConfirmForce] = useState(false);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    void (async () => {
      try {
        if (target.mode === "edit") {
          const { data } = await editableKube({ path: { env, kind: target.kind, ns: target.ns, name: target.name }, throwOnError: true });
          setText(data.yaml);
          setLoaded(data.yaml);
          setRv(data.resource_version);
        } else if (target.mode === "create") {
          const { data } = await kubeTemplates({ path: { env }, throwOnError: true });
          const t = (data[target.kind] ?? "").replaceAll("NAMESPACE", target.namespace);
          setText(t);
          setLoaded(t);
        }
      } catch (e) {
        setProblems([errorMessage(e)]);
      }
    })();
  }, [env, target]);

  /** Fetches the object as it is now, for the conflict view. */
  const live = async () => {
    if (target.mode !== "edit") return;
    const { data } = await editableKube({ path: { env, kind: target.kind, ns: target.ns, name: target.name }, throwOnError: true });
    setConflict({ yaml: data.yaml, rv: data.resource_version });
  };

  /** The results, or null when an edit hit a conflict. */
  const send = async (dryRun: boolean, force: boolean, version: string): Promise<ApplyResult[] | null> => {
    if (target.mode === "edit") {
      try {
        const { data } = await replaceKube({
          path: { env, kind: target.kind, ns: target.ns, name: target.name },
          body: { yaml: text, resource_version: version, dry_run: dryRun },
          throwOnError: true,
        });
        return [data];
      } catch (e) {
        if (e instanceof ApiError && e.status === 409) {
          await live();
          return null;
        }
        throw e;
      }
    }
    const { data } = await applyKube({
      path: { env },
      body: { yaml: text, namespace: target.namespace, dry_run: dryRun, force },
      throwOnError: true,
    });
    return data;
  };

  const run = async (dryRun: boolean, force = false, version = rv) => {
    const errs = yamlErrors(text);
    if (errs.length) return setProblems(errs);
    setProblems([]);
    setBusy(true);
    try {
      const out = await send(dryRun, force, version);
      if (!out) return;
      setResults(out);
      if (dryRun) return setStage("review");
      await qc.invalidateQueries({ queryKey: ["env", env, "k8s"] });
      if (out.some((r) => r.error)) return setStage("failed");
      toast.success(out.length === 1 ? `${out[0].kind} ${out[0].name} ${out[0].action}` : `${out.length} objects applied`);
      onClose();
    } catch (e) {
      setProblems([errorMessage(e)]);
    } finally {
      setBusy(false);
    }
  };

  const close = () => {
    if (text !== loaded && stage !== "failed" && !window.confirm("Discard your changes?")) return;
    onClose();
  };

  const forceable = target.mode !== "edit" && results.some((r) => r.error?.code === "conflict");

  return (
    <Dialog open onOpenChange={(o) => !o && close()}>
      <DialogContent className="max-w-4xl">
        <DialogTitle>{title(target)}</DialogTitle>
        <DialogDescription>
          {stage === "edit" ? "Review shows what would change before anything is written." : "Nothing more is written until you apply."}
        </DialogDescription>
        {problems.length > 0 && (
          <ul role="alert" className="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm">
            {problems.map((p) => (
              <li key={p}>{p}</li>
            ))}
          </ul>
        )}
        {conflict ? (
          <div className="grid gap-2">
            <p className="text-sm">This object changed since you opened it.</p>
            <div className="grid gap-2 md:grid-cols-2">
              <pre aria-label="Your edit" className="max-h-[40vh] overflow-auto rounded-md border p-2 font-mono text-xs">
                {text}
              </pre>
              <pre aria-label="Live YAML" className="max-h-[40vh] overflow-auto rounded-md border p-2 font-mono text-xs">
                {conflict.yaml}
              </pre>
            </div>
            <div className="flex justify-end gap-2">
              <Button
                variant="outline"
                onClick={() => {
                  setText(conflict.yaml);
                  setLoaded(conflict.yaml);
                  setRv(conflict.rv);
                  setConflict(null);
                  setProblems([]);
                  setStage("edit");
                }}
              >
                Discard
              </Button>
              <Button
                onClick={() => {
                  const v = conflict.rv;
                  setRv(v);
                  setConflict(null);
                  void run(true, false, v);
                }}
              >
                Reload and reapply my edit
              </Button>
            </div>
          </div>
        ) : stage === "edit" ? (
          <>
            <Suspense fallback={<p className="text-sm text-muted-foreground">Loading editor…</p>}>
              <CodeMirrorField label="YAML" value={text} onChange={setText} />
            </Suspense>
            <p className="text-xs text-muted-foreground">
              Secret values are hidden; leave them unchanged to keep them. Ctrl+M (Shift+Alt+M on macOS) lets Tab leave the editor.
            </p>
            <div className="flex justify-end gap-2">
              <Button variant="outline" onClick={close}>
                Cancel
              </Button>
              <Button disabled={busy || !text.trim()} onClick={() => void run(true)}>
                Review
              </Button>
            </div>
          </>
        ) : (
          <>
            <ul className="grid gap-3">
              {results.map((r) => (
                <li key={r.index} className="grid gap-1">
                  <p className="text-sm font-medium">
                    {r.kind} {r.namespace ? `${r.namespace}/` : ""}
                    {r.name} <span className="text-muted-foreground">{r.error ? "failed" : r.action}</span>
                  </p>
                  {r.error ? (
                    <p className="text-sm text-destructive">{r.error.message}</p>
                  ) : (
                    <DiffView label={`Changes to ${r.kind} ${r.name}`} before={r.before} after={r.after} />
                  )}
                </li>
              ))}
            </ul>
            <div className="flex justify-end gap-2">
              <Button variant="outline" onClick={() => setStage("edit")}>
                Back to editing
              </Button>
              {forceable && (
                <Button variant="outline" onClick={() => setConfirmForce(true)}>
                  Force apply
                </Button>
              )}
              {stage === "review" && (
                <Button disabled={busy} onClick={() => void run(false)}>
                  Apply
                </Button>
              )}
            </div>
          </>
        )}
        {confirmForce && (
          <ConfirmDialog
            open
            onOpenChange={(o) => !o && setConfirmForce(false)}
            title="Force apply?"
            description="FleetOps takes ownership of the conflicting fields. The other manager may change them back."
            confirmLabel="Force apply anyway"
            onConfirm={() => run(false, true)}
          />
        )}
      </DialogContent>
    </Dialog>
  );
}
