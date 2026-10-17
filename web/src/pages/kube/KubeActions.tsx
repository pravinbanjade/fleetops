import { useNavigate } from "@tanstack/react-router";
import { lazy, Suspense, useState } from "react";
import type { KubeKind, KubeRow } from "@/api/gen/types.gen";
import { useEnvAccess } from "@/api/access";
import { actPerm, kubePerm } from "./perms";
import { Button } from "@/components/ui/button";
import { ActDialog, actIcon, actLabel, actsFor, type Act } from "./Act";
import { READ_ONLY } from "./kinds";

const YamlDialog = lazy(() => import("@/components/yaml-editor/YamlDialog"));

/** The object page's actions: the same set a list offers for a selection of one. */
export function KubeActions({ env, kind, ns, name, row }: { env: string; kind: KubeKind; ns: string; name: string; row: KubeRow }) {
  const navigate = useNavigate();
  const { can } = useEnvAccess(env);
  const [open, setOpen] = useState<Act | "edit" | null>(null);
  const where = ns === "-" ? null : ns;
  const canEdit = can(kubePerm(kind, "operate"), where);
  const acts = actsFor(kind, [row])
    .map((a) => a.act)
    .filter((act) => can(actPerm(kind, act), where));
  if (READ_ONLY.includes(kind) || (!canEdit && acts.length === 0)) return null;
  const done = (act: Act, results: (string | undefined)[]) => {
    setOpen(null);
    if (act === "delete") void navigate({ to: "/envs/$env/k8s/$kind", params: { env, kind } });
    // Run now: the new Job is what you came to watch.
    const job = results[0];
    if (act === "trigger" && job) void navigate({ to: "/envs/$env/k8s/$kind/$ns/$name", params: { env, kind: "jobs", ns, name: job } });
  };
  return (
    <div className="flex flex-wrap gap-2">
      {canEdit && (
        <Button variant="outline" size="sm" onClick={() => setOpen("edit")}>
          Edit YAML
        </Button>
      )}
      {acts.map((act) => {
        const Icon = actIcon(act);
        return (
          <Button key={act} variant="outline" size="sm" onClick={() => setOpen(act)}>
            <Icon aria-hidden />
            {actLabel(act)}
          </Button>
        );
      })}
      {open === "edit" && (
        <Suspense fallback={null}>
          <YamlDialog env={env} target={{ mode: "edit", kind, ns, name }} onClose={() => setOpen(null)} />
        </Suspense>
      )}
      {open && open !== "edit" && (
        <ActDialog env={env} act={open} targets={[{ kind, ns, name, row }]} onClose={() => setOpen(null)} onDone={(r) => done(open, r)} />
      )}
    </div>
  );
}
