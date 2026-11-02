import { useQuery } from "@tanstack/react-query";
import { CircleArrowUp, RotateCcw, Trash2 } from "lucide-react";
import { lazy, Suspense, useState } from "react";
import { useEnvAccess } from "@/api/access";
import type { HelmRelease } from "@/api/gen/types.gen";
import { helmCapabilitiesQuery, helmHistoryQuery, helmReleaseQuery } from "@/api/queries";
import { ObjectNow } from "@/components/shell/ObjectNow";
import { Button } from "@/components/ui/button";
import { formatAgo } from "@/lib/format";
import type { HelmAction } from "./HelmPlanDialog";
import { HelmStatus } from "./HelmStatus";

const HelmPlanDialog = lazy(() => import("./HelmPlanDialog"));
const HelmDeployDialog = lazy(() => import("./HelmDeployDialog"));

/**
 * A Helm release in the panel: its chart, revision and status, the error
 * that failed it, and what an operator does next: Upgrade, Roll back to the
 * revision before this one, Uninstall. Each shows its plan before it runs.
 */
export function ReleaseNow({ env, release: r }: { env: string; release: HelmRelease }) {
  const { can } = useEnvAccess(env);
  const operate = can("helm.operate", r.namespace);
  const remove = can("helm.delete", r.namespace);
  const caps = useQuery({ ...helmCapabilitiesQuery(env), enabled: operate });
  const detail = useQuery({ ...helmReleaseQuery(env, r.namespace, r.name), enabled: operate });
  const history = useQuery({ ...helmHistoryQuery(env, r.namespace, r.name), enabled: operate });
  const [action, setAction] = useState<HelmAction | null>(null);
  const [upgrading, setUpgrading] = useState(false);
  // The newest revision before this one that can still be read back.
  const previous = (history.data ?? [])
    .filter((h) => h.revision < r.revision && h.status !== "unreadable")
    .reduce<number | undefined>((best, h) => (best === undefined || h.revision > best ? h.revision : best), undefined);
  // Upgrade starts from the current values, so it needs to see them.
  const canUpgrade = operate && !!caps.data?.install && !!detail.data && detail.data.hidden !== true;
  return (
    <ObjectNow
      title={r.name}
      subtitle={`${r.chart} ${r.chart_version} · app ${r.app_version} · ${r.namespace}`}
      facts={[
        { label: "Status", value: <HelmStatus status={r.status} /> },
        { label: "Revision", value: <span className="font-figure">{r.revision}</span> },
        { label: "Updated", value: formatAgo(r.updated) },
      ]}
      open={{ to: "/envs/$env/k8s/helm/$ns/$name", params: { env, ns: r.namespace, name: r.name } }}
      actions={
        <>
          {canUpgrade && (
            <Button size="sm" variant="secondary" onClick={() => setUpgrading(true)}>
              <CircleArrowUp aria-hidden />
              Upgrade
            </Button>
          )}
          {operate && previous !== undefined && (
            <Button size="sm" variant="secondary" onClick={() => setAction({ kind: "rollback", revision: previous })}>
              <RotateCcw aria-hidden />
              Roll back to {previous}
            </Button>
          )}
          {remove && (
            <Button size="sm" variant="destructive" onClick={() => setAction({ kind: "uninstall" })}>
              <Trash2 aria-hidden />
              Uninstall
            </Button>
          )}
          {upgrading && detail.data && (
            <Suspense fallback={null}>
              <HelmDeployDialog
                env={env}
                target={{ kind: "upgrade", ns: r.namespace, name: r.name, source: detail.data.chart_source ?? null, values: detail.data.values ?? "" }}
                onClose={() => setUpgrading(false)}
              />
            </Suspense>
          )}
          {action && (
            <Suspense fallback={null}>
              <HelmPlanDialog env={env} ns={r.namespace} name={r.name} action={action} onClose={() => setAction(null)} />
            </Suspense>
          )}
        </>
      }
    >
      {r.error && <p className="rounded-md bg-raised p-3 font-ident text-sm text-fall">{r.error}</p>}
    </ObjectNow>
  );
}
