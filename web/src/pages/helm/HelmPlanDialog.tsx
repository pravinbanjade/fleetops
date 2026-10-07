import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { rollbackHelmRelease, uninstallHelmRelease } from "@/api/gen/sdk.gen";
import type { HelmPlan } from "@/api/gen/types.gen";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { DiffView } from "@/components/yaml-editor/DiffView";
import { errorMessage } from "@/lib/errors";

export type HelmAction = { kind: "rollback"; revision: number } | { kind: "uninstall" };

/** Plans a rollback or uninstall (a dry run), shows it, and commits against the reviewed revision. */
export default function HelmPlanDialog({
  env,
  ns,
  name,
  action,
  onClose,
}: {
  env: string;
  ns: string;
  name: string;
  action: HelmAction;
  onClose: () => void;
}) {
  const qc = useQueryClient();
  const [typed, setTyped] = useState("");
  const [anyway, setAnyway] = useState(false);
  const send = async (dry_run: boolean, expected_current: number): Promise<HelmPlan> => {
    const path = { env, ns, name };
    const r =
      action.kind === "rollback"
        ? await rollbackHelmRelease({ path, body: { revision: action.revision, expected_current, dry_run } })
        : await uninstallHelmRelease({ path, body: { expected_current, dry_run } });
    return r.data;
  };
  const review = useMutation({ mutationFn: () => send(true, 0), meta: { inline: true } });
  const commit = useMutation({
    mutationFn: (current: number) => send(false, current),
    meta: { inline: true },
    onSettled: () => qc.invalidateQueries({ queryKey: ["env", env, "helm"] }),
  });
  const { mutate: plan } = review;
  useEffect(() => plan(), [plan]);
  const shown = commit.data ?? review.data;
  const verb = action.kind === "rollback" ? "Roll back" : "Uninstall";
  const title = action.kind === "rollback" ? `Roll back ${name} to revision ${action.revision}` : `Uninstall ${name}`;
  // A plan the cluster already refuses in part would leave the release
  // half-changed: committing it takes an explicit choice.
  const failing = review.data?.changes.filter((c) => c.error).length ?? 0;
  const ready =
    !!review.data && !commit.data && (action.kind === "rollback" || typed === name) && (failing === 0 || anyway);
  const changed = !!commit.error && errorMessage(commit.error).includes("review again");
  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="max-w-3xl">
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
          <DialogDescription>{shown?.note ?? "Working out what would change…"}</DialogDescription>
        </DialogHeader>
        {review.error && (
          <p role="alert" className="text-sm text-state-fail">
            {errorMessage(review.error)}
          </p>
        )}
        {commit.data && (
          <p role="status" className="text-sm">
            {action.kind === "rollback"
              ? `Revision ${commit.data.revision} is ${commit.data.status}.`
              : commit.data.status === "uninstalled"
                ? `${name} is uninstalled.`
                : `Uninstall ${commit.data.status}.`}
          </p>
        )}
        <ul className="grid max-h-[55vh] gap-3 overflow-auto">
          {shown?.changes.map((c) => (
            <li key={`${c.kind}/${c.namespace ?? "-"}/${c.name}`} className="grid gap-1">
              <span className="text-sm font-medium">
                {c.change} {c.kind} {c.name}
              </span>
              {c.error ? (
                <span role="alert" className="text-sm text-state-fail">
                  {c.error.message}
                </span>
              ) : (
                c.change !== "keep" && <DiffView label={`${c.kind} ${c.name}`} before={c.before} after={c.after} />
              )}
            </li>
          ))}
        </ul>
        {failing > 0 && !commit.data && (
          <div className="flex items-center gap-2 text-sm">
            <Checkbox id="helm-anyway" checked={anyway} onCheckedChange={(v) => setAnyway(v === true)} />
            <Label htmlFor="helm-anyway">
              Commit anyway: {failing} object{failing === 1 ? "" : "s"} will fail and the release will be marked failed
            </Label>
          </div>
        )}
        {action.kind === "uninstall" && review.data && !commit.data && (
          <div className="grid gap-1">
            <Label htmlFor="helm-confirm">Type {name} to confirm</Label>
            <Input id="helm-confirm" value={typed} onChange={(e) => setTyped(e.target.value)} autoComplete="off" />
          </div>
        )}
        {commit.error && (
          <p role="alert" className="text-sm text-state-fail">
            {errorMessage(commit.error)}
          </p>
        )}
        <DialogFooter>
          {changed && (
            <Button
              variant="outline"
              onClick={() => {
                commit.reset();
                review.mutate();
              }}
            >
              Review again
            </Button>
          )}
          <Button variant="outline" onClick={onClose}>
            {commit.data ? "Close" : "Cancel"}
          </Button>
          {!commit.data && (
            <Button
              variant={action.kind === "uninstall" ? "destructive" : "default"}
              disabled={!ready || commit.isPending}
              onClick={() => review.data && commit.mutate(review.data.current)}
            >
              {verb}
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
