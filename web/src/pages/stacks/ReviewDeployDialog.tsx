import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { deployStack, planStack } from "@/api/gen/sdk.gen";
import type { DeploySource, ServicePlan } from "@/api/gen/types.gen";
import { ApiError } from "@/api/client";
import { keys, swarmKeys } from "@/api/queries";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { DiffView } from "@/components/yaml-editor/DiffView";
import { errorMessage } from "@/lib/errors";
import { useTask, useTaskTracker } from "@/live/TaskProvider";
import { TaskView } from "@/pages/images/TaskDialogs";
import { IssueLine } from "./SecretsTab";

const ACTION_LABEL: Record<ServicePlan["action"], string> = {
  create: "Create",
  recreate: "Recreate",
  update: "Update",
  scale: "Scale",
  remove: "Remove",
  unchanged: "Unchanged",
};
const ORDER: ServicePlan["action"][] = ["recreate", "update", "create", "scale", "remove", "unchanged"];

/** Plans a deploy of the draft or an earlier revision, shows it, and deploys exactly that plan. */
export function ReviewDeployDialog({
  env,
  stack,
  swarm = false,
  source,
  onClose,
  onSecrets,
}: {
  env: string;
  stack: string;
  /** A Swarm stack: Swarm rolls it out, and a failed deploy is not undone. */
  swarm?: boolean;
  source: DeploySource;
  onClose: () => void;
  /** Opens the stack's Secrets tab, where a missing value is set. */
  onSecrets?: () => void;
}) {
  const qc = useQueryClient();
  const tracker = useTaskTracker();
  const [taskId, setTaskId] = useState<string | null>(null);
  const task = useTask(taskId);
  const plan = useQuery({
    queryKey: [...keys.stack(env, stack), "plan", source],
    queryFn: async () => (await planStack({ path: { env, stack }, body: source })).data,
    staleTime: Infinity,
    gcTime: 0,
  });
  const deploy = useMutation({
    meta: { inline: true },
    mutationFn: async (token: string) =>
      (await deployStack({ path: { env, stack }, body: { from: source, plan_token: token } })).data,
    onSuccess: (d) => {
      tracker.track(d.task_id, env, `Deploy ${stack} revision ${d.revision}`);
      setTaskId(d.task_id);
    },
  });
  const close = () => {
    void qc.invalidateQueries({ queryKey: keys.stack(env, stack) });
    void qc.invalidateQueries({ queryKey: keys.stacks(env) });
    void qc.invalidateQueries({ queryKey: swarm ? swarmKeys.all(env) : keys.containers(env) });
    onClose();
  };
  const p = plan.data;
  const stale = deploy.error instanceof ApiError && deploy.error.status === 409;
  const blocked = !p?.plan || p.issues.length > 0 || p.plan.blocking.length > 0;
  const title = source.source === "revision" ? `Roll back ${stack} to revision ${source.revision}` : `Deploy ${stack}`;
  return (
    <Dialog open onOpenChange={(o) => !o && close()}>
      <DialogContent className="sm:max-w-3xl">
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
          <DialogDescription>
            {swarm
              ? "Swarm rolls each service out with its update policy. If the deploy fails, what was applied stays; roll back to an earlier revision to undo it."
              : "What the deploy will change. If it fails part way, the stack is put back as it was."}
          </DialogDescription>
        </DialogHeader>
        {task ? (
          <TaskView task={task} onClose={close}>
            <ol aria-label="Deploy log" className="max-h-[40vh] overflow-auto rounded-md border bg-muted/40 p-3 font-mono text-xs">
              {task.messages.map((m, i) => {
                const msg = m as { step?: string; message?: string; error?: string };
                return (
                  <li key={i} className={msg.error ? "text-destructive" : undefined}>
                    {msg.error ?? `${msg.step}: ${msg.message}`}
                  </li>
                );
              })}
            </ol>
          </TaskView>
        ) : plan.error ? (
          <p role="alert" className="text-sm text-destructive">
            {errorMessage(plan.error)}
          </p>
        ) : !p ? (
          <p className="text-sm text-muted-foreground">Working out the plan…</p>
        ) : (
          <div className="grid max-h-[65vh] gap-4 overflow-auto">
            {p.issues.length > 0 && (
              <section aria-label="Problems" className="grid gap-1 text-sm text-destructive">
                <h3 className="font-semibold">Fix these first</h3>
                <ul className="grid gap-0.5">
                  {p.issues.map((i, n) => (
                    <li key={n}>
                      <IssueLine issue={i} onSecrets={onSecrets} />
                    </li>
                  ))}
                </ul>
              </section>
            )}
            {p.plan && p.plan.blocking.length > 0 && (
              <ul aria-label="Blocking" className="grid gap-0.5 text-sm text-destructive">
                {p.plan.blocking.map((b) => (
                  <li key={b}>{b}</li>
                ))}
              </ul>
            )}
            {p.plan && <PlanView plan={p.plan} />}
            {p.warnings.length > 0 && (
              <ul aria-label="Warnings" className="grid gap-0.5 text-sm text-muted-foreground">
                {p.warnings.map((w, n) => (
                  <li key={n}>
                    <span className="font-ident">{w.path}</span>: {w.message}
                  </li>
                ))}
              </ul>
            )}
            <DiffView label="Compose file changes" before={p.compose_before} after={p.compose_after} />
            {p.variables_changed.length > 0 && (
              <p className="text-sm">
                Variables changed: <span className="font-ident">{p.variables_changed.join(", ")}</span>
              </p>
            )}
          </div>
        )}
        {!task && (
          <>
            {deploy.error && (
              <p role="alert" className="text-sm text-destructive">
                {errorMessage(deploy.error)}
              </p>
            )}
            <DialogFooter>
              <Button variant="outline" onClick={close}>
                Cancel
              </Button>
              {stale ? (
                <Button
                  onClick={() => {
                    deploy.reset();
                    void plan.refetch();
                  }}
                >
                  Review again
                </Button>
              ) : (
                <Button disabled={blocked || deploy.isPending} onClick={() => p?.plan && deploy.mutate(p.plan.token)}>
                  {source.source === "revision" ? "Roll back" : "Deploy"}
                </Button>
              )}
            </DialogFooter>
          </>
        )}
      </DialogContent>
    </Dialog>
  );
}

function PlanView({ plan }: { plan: NonNullable<import("@/api/gen/types.gen").PlanResponse["plan"]> }) {
  const groups = ORDER.map((a) => [a, plan.services.filter((s) => s.action === a)] as const).filter(([, s]) => s.length);
  const pulls = plan.images.filter((i) => i.pull);
  const builds = plan.images.filter((i) => i.build);
  const resources = [
    ...plan.networks.map((n) => ({ ...n, kind: "network" })),
    ...plan.volumes.map((v) => ({ ...v, kind: "volume" })),
    ...plan.secrets.map((s) => ({ ...s, kind: "secret" })),
    ...plan.configs.map((c) => ({ ...c, kind: "config" })),
  ].filter((r) => r.action !== "exists");
  return (
    <section aria-label="Plan" className="grid gap-3 text-sm">
      {groups.map(([action, services]) => (
        <div key={action} className="grid gap-1">
          <h3 className="font-semibold">{ACTION_LABEL[action]}</h3>
          <ul className="grid gap-1">
            {services.map((s) => (
              <li key={s.name}>
                <span className="font-ident font-medium">{s.name}</span>
                {s.action === "scale" && ` (${s.replicas_before} → ${s.replicas_after})`}
                {s.reasons.length > 0 && action !== "unchanged" && (
                  <ul className="ml-4 list-disc text-muted-foreground">
                    {s.reasons.map((r) => (
                      <li key={r}>{r}</li>
                    ))}
                  </ul>
                )}
                {s.drift.length > 0 && <p className="ml-4 text-state-wait-text">Changed outside the stack: {s.drift.join(", ")}</p>}
              </li>
            ))}
          </ul>
        </div>
      ))}
      {resources.length > 0 && (
        <ul aria-label="Networks and volumes" className="grid gap-0.5">
          {resources.map((r) => (
            <li key={`${r.kind}-${r.name}`}>
              {r.action === "kept" ? "Keep" : r.action === "create" ? "Create" : "Remove"} {r.kind}{" "}
              <span className="font-ident">{r.name}</span>
              {r.action === "kept" && <span className="text-muted-foreground"> (no longer in the file; data is kept)</span>}
            </li>
          ))}
        </ul>
      )}
      {pulls.length > 0 && (
        <p>
          Pull: <span className="font-ident">{pulls.map((i) => i.image).join(", ")}</span>
        </p>
      )}
      {builds.length > 0 && (
        <p>
          Build: <span className="font-ident">{builds.map((i) => i.image).join(", ")}</span>
        </p>
      )}
    </section>
  );
}
