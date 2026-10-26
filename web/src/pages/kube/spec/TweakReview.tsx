import { useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowLeft, ArrowRight, Check, CircleAlert } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import { ApiError } from "@/api/client";
import { editableKube, replaceKube } from "@/api/gen/sdk.gen";
import type { ApplyResult, KubeKind } from "@/api/gen/types.gen";
import { meQuery } from "@/api/queries";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogTitle } from "@/components/ui/dialog";
import { DiffView } from "@/components/yaml-editor/DiffView";
import { errorMessage } from "@/lib/errors";
import { KIND_LABEL } from "../kinds";
import { applyOps, describeOp, TWEAKS, type ObjectSpec, type Op } from "./model";

type Prepared = { yaml: string; rv: string; result: ApplyResult; moved: boolean };

/** What applying will do to running pods, said before it happens. */
function consequence(kind: KubeKind, revert: boolean, managedBy?: string): string[] {
  const out: string[] = [];
  if (kind === "configmaps" || kind === "secrets")
    out.push(
      "Pods that read it as environment variables keep their old values until they restart. Pods that mount it as files see the change within a minute or two, unless they mount single keys with subPath.",
    );
  else if (kind === "cronjobs") out.push("Jobs it starts from now on get the new environment; Jobs already running keep theirs.");
  else out.push("This starts a rollout: the pods are replaced with ones that have the new environment.");
  if (kind === "secrets") out.push("Secret values are not recorded, so this change cannot be reverted from FleetOps.");
  else if (revert) out.push("Each value goes back to what it was before it was first changed from FleetOps.");
  else out.push(`The values these replace are kept in the ${TWEAKS} annotation, so you can revert them from this page.`);
  if (managedBy) out.push(`${managedBy} manages this object: its next sync or upgrade may set these values back.`);
  return out;
}

/**
 * Reviews a tweak before it is written: the object is read fresh, the
 * changes are applied to it and sent as a dry run, and nothing is written
 * until Apply. A change made meanwhile is folded in and reviewed again.
 */
export function TweakReview({
  env,
  kind,
  ns,
  name,
  ops,
  spec,
  revert,
  onClose,
  onApplied,
}: {
  env: string;
  kind: KubeKind;
  ns: string;
  name: string;
  ops: Op[];
  spec: ObjectSpec;
  revert?: boolean;
  onClose: () => void;
  onApplied: () => void;
}) {
  const qc = useQueryClient();
  const me = useQuery(meQuery).data?.user.username;
  const [attempt, setAttempt] = useState(0);
  const [problem, setProblem] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const path = { env, kind, ns, name };

  // Read fresh, apply the ops, dry-run. A conflict on Apply bumps the attempt,
  // which folds the same ops into the newer version and reviews again.
  const prep = useQuery({
    queryKey: ["tweak-review", env, kind, ns, name, attempt],
    queryFn: async (): Promise<Prepared> => {
      const { data } = await editableKube({ path, throwOnError: true });
      const yaml = applyOps(kind, data.yaml, ops, me);
      const { data: result } = await replaceKube({ path, body: { yaml, resource_version: data.resource_version, dry_run: true }, throwOnError: true });
      return { yaml, rv: data.resource_version, result, moved: attempt > 0 };
    },
    staleTime: Infinity,
    gcTime: 0,
    retry: false,
    refetchOnWindowFocus: false,
  });
  const prepared = prep.data;

  const apply = async () => {
    if (!prepared) return;
    setBusy(true);
    try {
      await replaceKube({ path, body: { yaml: prepared.yaml, resource_version: prepared.rv, dry_run: false }, throwOnError: true });
      await qc.invalidateQueries({ queryKey: ["env", env, "k8s"] });
      toast.success(`${KIND_LABEL[kind].replace(/s$/, "")} ${name} ${revert ? "reverted" : "updated"}`);
      onApplied();
    } catch (e) {
      if (e instanceof ApiError && e.status === 409) setAttempt((a) => a + 1);
      else setProblem(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const secret = kind === "secrets";
  const failed = prepared?.result.error;
  const error = problem ?? (prep.error ? errorMessage(prep.error) : null);
  const unchanged = prepared && !failed && prepared.result.action === "unchanged";
  const changes = ops.map((op) => ({ op, ...describeOp(op, spec) }));

  return (
    <Dialog open onOpenChange={(o) => !o && !busy && onClose()}>
      <DialogContent className="max-w-2xl">
        <DialogTitle>{revert ? `Revert ${name}?` : `Review changes to ${name}`}</DialogTitle>
        <DialogDescription>Nothing is written until you apply.</DialogDescription>

        <ul aria-label="Changes" className="font-strip overflow-hidden rounded-md bg-[#1c1c1c] text-sm">
          {changes.map(({ op, verb, subject, where, before, after }) => (
            <li key={`${op.op}:${op.op === "env" ? `${op.container}/${op.name}` : op.key}`} className="grid gap-1 border-b px-3 py-2.5 last:border-b-0">
              <span className="font-medium">
                {verb} <span className="font-ident">{subject}</span>
                {where && (
                  <span className="font-normal text-muted-foreground">
                    {" "}
                    in <span className="font-ident">{where}</span>
                  </span>
                )}
              </span>
              {(before !== undefined || after !== undefined) && (
                <span className="flex min-w-0 flex-wrap items-baseline gap-x-2 gap-y-1 font-ident text-xs font-normal">
                  {before !== undefined && <del className="max-w-full truncate text-muted-foreground">{secret ? "the current value" : before || "(empty)"}</del>}
                  {before !== undefined && after !== undefined && <ArrowRight aria-label="to" className="size-3.5 self-center text-muted-foreground" />}
                  {after !== undefined && <ins className="max-w-full truncate no-underline">{secret ? "a new value" : after || "(empty)"}</ins>}
                </span>
              )}
            </li>
          ))}
        </ul>

        <ul className="grid list-disc gap-1 pl-5 text-sm text-muted-foreground">
          {consequence(kind, !!revert, spec.managedBy).map((c) => (
            <li key={c}>{c}</li>
          ))}
        </ul>

        {prepared?.moved && (
          <p role="status" className="text-sm">
            {KIND_LABEL[kind].replace(/s$/, "")} {name} changed while you were reviewing. Your changes are now applied to the new version; check them again.
          </p>
        )}
        {(error || failed) && (
          <div role="alert" className="flex gap-2 rounded-md bg-destructive/8 px-3 py-2 text-sm">
            <CircleAlert aria-hidden className="mt-0.5 size-4 shrink-0 text-destructive" />
            <p className="min-w-0 font-mono text-xs break-words">{error ?? failed?.message}</p>
          </div>
        )}
        {prep.isFetching && (
          <p role="status" className="text-sm text-muted-foreground">
            Checking the change with the cluster…
          </p>
        )}
        {unchanged && <p className="text-sm text-muted-foreground">The cluster reports nothing would change.</p>}
        {prepared && !failed && !unchanged && (
          <details className="group text-sm">
            <summary className="cursor-pointer text-muted-foreground select-none hover:text-foreground">Show the YAML diff</summary>
            <div className="mt-2">
              <DiffView label={`Changes to ${name}`} before={prepared.result.before} after={prepared.result.after} />
            </div>
          </details>
        )}

        <div className="flex justify-end gap-2">
          <Button variant="outline" disabled={busy} onClick={onClose}>
            <ArrowLeft aria-hidden />
            {revert ? "Cancel" : "Back to editing"}
          </Button>
          <Button disabled={busy || !prepared || !!failed || !!unchanged} onClick={() => void apply()}>
            <Check aria-hidden />
            {busy ? "Applying…" : revert ? "Revert" : "Apply"}
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
