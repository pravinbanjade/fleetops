import { useMutation, useQueryClient } from "@tanstack/react-query";
import { lazy, Suspense, useState } from "react";
import { installHelmChart, showHelmChart, upgradeHelmRelease } from "@/api/gen/sdk.gen";
import type { ChartRef, HelmPlan } from "@/api/gen/types.gen";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { DiffView } from "@/components/yaml-editor/DiffView";
import { errorMessage } from "@/lib/errors";

const CodeMirrorField = lazy(() => import("@/components/yaml-editor/CodeMirrorField"));

export type DeployTarget =
  | { kind: "install"; ns: string }
  | { kind: "upgrade"; ns: string; name: string; source: ChartRef | null; values: string };

type Source = "repo" | "oci";

/** Install a chart or upgrade a release: the form, a reviewed plan, then the commit (spec 3j §6). */
export default function HelmDeployDialog({ env, target, onClose }: { env: string; target: DeployTarget; onClose: () => void }) {
  const qc = useQueryClient();
  const initial = target.kind === "upgrade" ? target.source : null;
  const [name, setName] = useState(target.kind === "upgrade" ? target.name : "");
  const [ns, setNs] = useState(target.ns);
  const [source, setSource] = useState<Source>(initial?.oci ? "oci" : "repo");
  const [repoUrl, setRepoUrl] = useState(initial?.repo_url ?? "");
  const [chartName, setChartName] = useState(initial?.chart ?? "");
  const [oci, setOci] = useState(initial?.oci ?? "");
  const [version, setVersion] = useState(initial?.version ?? "");
  const [values, setValues] = useState(target.kind === "upgrade" ? target.values : "");
  const chart: ChartRef =
    source === "oci"
      ? { oci: oci.trim(), version: version.trim() || null }
      : { repo_url: repoUrl.trim(), chart: chartName.trim(), version: version.trim() || null };
  const send = async (dry_run: boolean, expected_current: number): Promise<HelmPlan> => {
    if (target.kind === "install") {
      return (await installHelmChart({ path: { env, ns }, body: { name, chart, values, dry_run, expected_current } })).data;
    }
    return (await upgradeHelmRelease({ path: { env, ns, name }, body: { chart, values, dry_run, expected_current } })).data;
  };
  const review = useMutation({ mutationFn: () => send(true, 0), meta: { inline: true } });
  const commit = useMutation({
    mutationFn: (current: number) => send(false, current),
    meta: { inline: true },
    onSettled: () => qc.invalidateQueries({ queryKey: ["env", env, "helm"] }),
  });
  const defaults = useMutation({ mutationFn: async () => (await showHelmChart({ path: { env }, body: { chart } })).data, meta: { inline: true } });
  const shown = commit.data ?? review.data;
  const verb = target.kind === "install" ? "Install" : "Upgrade";
  const title = target.kind === "install" ? "Install a chart" : `Upgrade ${name}`;
  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="max-w-4xl">
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
          <DialogDescription>Rendered by the server's helm; nothing changes until you confirm.</DialogDescription>
        </DialogHeader>
        {!review.data && (
          <div className="grid gap-3">
            {target.kind === "install" && (
              <div className="grid grid-cols-2 gap-3">
                <Field id="hd-name" label="Release name" value={name} onChange={setName} />
                <Field id="hd-ns" label="Namespace" value={ns} onChange={setNs} />
              </div>
            )}
            <div role="radiogroup" aria-label="Chart source" className="flex gap-4 text-sm">
              {(["repo", "oci"] as const).map((s) => (
                <label key={s} className="flex items-center gap-2">
                  <input type="radio" name="hd-source" checked={source === s} onChange={() => setSource(s)} />
                  {s === "repo" ? "Repository" : "OCI registry"}
                </label>
              ))}
            </div>
            {source === "repo" ? (
              <div className="grid grid-cols-2 gap-3">
                <Field id="hd-repo" label="Repository URL" value={repoUrl} onChange={setRepoUrl} />
                <Field id="hd-chart" label="Chart" value={chartName} onChange={setChartName} />
              </div>
            ) : (
              <Field id="hd-oci" label="OCI reference" value={oci} onChange={setOci} />
            )}
            <Field id="hd-version" label="Version (empty for latest)" value={version} onChange={setVersion} />
            <div className="grid gap-3 md:grid-cols-2">
              <div className="grid gap-1">
                <Suspense fallback={null}>
                  <CodeMirrorField label="Values" value={values} onChange={setValues} />
                </Suspense>
              </div>
              <div className="grid content-start gap-1">
                <Button variant="outline" size="sm" onClick={() => defaults.mutate()} disabled={defaults.isPending}>
                  Load chart defaults
                </Button>
                {defaults.error && <p role="alert" className="text-sm text-state-fail">{errorMessage(defaults.error)}</p>}
                {defaults.data && (
                  <pre aria-label="Chart defaults" className="max-h-80 overflow-auto rounded-md border bg-muted/40 p-2 font-mono text-xs">
                    {defaults.data.values}
                  </pre>
                )}
              </div>
            </div>
          </div>
        )}
        {review.error && <p role="alert" className="text-sm text-state-fail">{errorMessage(review.error)}</p>}
        {shown && (
          <div className="grid max-h-[60vh] gap-4 overflow-auto">
            {shown.chart && (
              <p className="text-sm">
                {shown.chart.name}-{shown.chart.version} (app {shown.chart.app_version})
              </p>
            )}
            {(shown.hooks?.length ?? 0) > 0 && (
              <section aria-label="Hooks" className="grid gap-1 text-sm">
                <h3 className="font-medium">Hooks, in order</h3>
                <ol className="list-decimal pl-5">
                  {(shown.hooks ?? []).map((h) => (
                    <li key={`${h.kind}/${h.name}`}>
                      {h.events.join(", ")} {h.kind} {h.name}
                    </li>
                  ))}
                </ol>
              </section>
            )}
            {shown.values_after != null && <DiffView label="Values" before={shown.values_before} after={shown.values_after} />}
            <ul className="grid gap-3">
              {shown.changes.map((c) => (
                <li key={`${c.kind}/${c.namespace ?? "-"}/${c.name}`} className="grid gap-1">
                  <span className="text-sm font-medium">
                    {c.change} {c.kind} {c.name}
                  </span>
                  {c.error ? (
                    <span role="alert" className="text-sm text-state-fail">{c.error.message}</span>
                  ) : (
                    <DiffView label={`${c.kind} ${c.name}`} before={c.before} after={c.after} />
                  )}
                </li>
              ))}
            </ul>
            {shown.notes && (
              <section aria-label="Notes">
                <h3 className="text-sm font-medium">Notes</h3>
                <pre className="whitespace-pre-wrap rounded-md border bg-muted/40 p-2 text-xs">{shown.notes}</pre>
              </section>
            )}
          </div>
        )}
        {commit.data && (
          <p role="status" className="text-sm">
            Revision {commit.data.revision} is {commit.data.status}.{commit.data.status === "failed" && ` ${commit.data.note}`}
          </p>
        )}
        {commit.error && <p role="alert" className="text-sm text-state-fail">{errorMessage(commit.error)}</p>}
        <DialogFooter>
          {review.data && !commit.data && (
            <Button variant="outline" onClick={() => review.reset()}>
              Back
            </Button>
          )}
          {!review.data ? (
            <Button onClick={() => review.mutate()} disabled={review.isPending || (target.kind === "install" && !name)}>
              Review
            </Button>
          ) : (
            !commit.data && (
              <Button onClick={() => commit.mutate(review.data!.current)} disabled={commit.isPending}>
                {verb}
              </Button>
            )
          )}
          {commit.data && <Button onClick={onClose}>Done</Button>}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function Field({ id, label, value, onChange }: { id: string; label: string; value: string; onChange: (v: string) => void }) {
  return (
    <div className="grid gap-1">
      <Label htmlFor={id}>{label}</Label>
      <Input id={id} value={value} onChange={(e) => onChange(e.target.value)} autoComplete="off" />
    </div>
  );
}
