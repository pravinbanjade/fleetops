import type { HelmRelease } from "@/api/gen/types.gen";
import { ObjectNow } from "@/components/shell/ObjectNow";
import { formatAgo } from "@/lib/format";
import { HelmStatus } from "./HelmStatus";

/** A Helm release in the panel: its chart, revision and status, and the error that failed it. */
export function ReleaseNow({ env, release: r }: { env: string; release: HelmRelease }) {
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
    >
      {r.error && <p className="rounded-md bg-raised p-3 font-ident text-sm text-fall">{r.error}</p>}
    </ObjectNow>
  );
}
