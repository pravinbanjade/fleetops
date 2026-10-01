import { useQuery } from "@tanstack/react-query";
import { envStatusQuery, kubeListQuery } from "@/api/queries";
import { Page } from "@/components/Page";

export function KubeOverviewPage({ env }: { env: string }) {
  const status = useQuery(envStatusQuery(env, true));
  const pods = useQuery(kubeListQuery(env, "pods"));
  const s = status.data;
  const version = s?.state === "up" && s.version.kind === "kube" ? s.version : undefined;
  const phases = new Map<string, number>();
  for (const p of pods.data?.rows ?? []) phases.set(p.status, (phases.get(p.status) ?? 0) + 1);
  return (
    <Page title="Overview">
      {s?.state === "down" && (
        <p role="alert" className="mb-4 rounded-md border border-state-fail/40 bg-state-fail/10 px-3 py-2 text-sm">
          {s.reason}
        </p>
      )}
      {version && (
        <dl className="grid gap-4 sm:grid-cols-3">
          <div className="rounded-lg border p-4">
            <dt className="text-xs text-muted-foreground">Kubernetes</dt>
            <dd className="font-mono text-lg">{version.git_version}</dd>
            <dd className="text-xs text-muted-foreground">{version.platform}</dd>
          </div>
          <div className="rounded-lg border p-4">
            <dt className="text-xs text-muted-foreground">Nodes</dt>
            <dd className="text-lg">
              {version.nodes == null ? "Not visible to these credentials" : `${version.nodes_ready} of ${version.nodes} nodes ready`}
            </dd>
          </div>
          <div className="rounded-lg border p-4">
            <dt className="text-xs text-muted-foreground">Pods</dt>
            <dd className="grid gap-0.5 text-sm">
              {[...phases].map(([phase, n]) => (
                <span key={phase}>{n} {phase}</span>
              ))}
            </dd>
          </div>
        </dl>
      )}
    </Page>
  );
}
