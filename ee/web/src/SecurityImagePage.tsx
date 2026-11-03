import { useMutation, useQuery } from "@tanstack/react-query";
import { Link, useSearch } from "@tanstack/react-router";
import { useState } from "react";
import { ScanSearch } from "lucide-react";
import type { DigestDetail, FindingView, Severity } from "@/api/gen/types.gen";
import { envsQuery } from "@/api/queries";
import { accessOf } from "@/api/access";
import { Page } from "@/components/Page";
import { Panel } from "@/components/Panel";
import { StateBadge } from "@/components/StateBadge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { NativeSelect } from "@/components/ui/native-select";
import { toast } from "sonner";
import { toastError } from "@/lib/errors";
import { formatAgo } from "@/lib/format";
import { imageQuery, scanNow, SEVERITY_LABEL, STATE_LABEL, STATE_TONE, useSecurityLive } from "./security";

const SEVERITY_TONE: Record<Severity, string> = {
  critical: "text-fall",
  high: "text-state-wait",
  medium: "text-ink",
  low: "text-ink-muted",
  unknown: "text-meta",
};

export function SeverityText({ severity }: { severity: Severity }) {
  return <span className={`font-medium ${SEVERITY_TONE[severity]}`}>{SEVERITY_LABEL[severity]}</span>;
}

/** Whether the caller may scan in each of `envs`. */
export function useCanScan(): (env: string) => boolean {
  const envs = useQuery(envsQuery).data ?? [];
  return (name) => {
    const e = envs.find((v) => v.name === name);
    if (!e) return false;
    const a = accessOf(e);
    return e.kind.startsWith("kube-") ? a.can("workloads.operate") : a.can("images.create", null);
  };
}

function Findings({ findings }: { findings: FindingView[] }) {
  const [min, setMin] = useState<Severity | "all">("all");
  const [fixOnly, setFixOnly] = useState(false);
  const order: Severity[] = ["unknown", "low", "medium", "high", "critical"];
  const shown = findings.filter((f) => (min === "all" || order.indexOf(f.severity) >= order.indexOf(min)) && (!fixOnly || f.fixed_version));
  return (
    <div className="grid gap-2">
      <div className="flex flex-wrap items-center gap-3 text-sm">
        <NativeSelect aria-label="Severity" value={min} onChange={(e) => setMin(e.target.value as Severity | "all")}>
          <option value="all">All severities</option>
          <option value="critical">Critical</option>
          <option value="high">High and above</option>
          <option value="medium">Medium and above</option>
        </NativeSelect>
        <label className="flex items-center gap-2">
          <Checkbox checked={fixOnly} onCheckedChange={(v) => setFixOnly(v === true)} />
          Fix available
        </label>
        <span className="text-meta">
          {shown.length} of {findings.length}
        </span>
      </div>
      <div className="overflow-x-auto">
        <table aria-label="Vulnerabilities" className="w-full text-sm">
          <thead className="text-left text-xs text-meta">
            <tr>
              <th className="py-1.5 pr-3 font-normal">ID</th>
              <th className="py-1.5 pr-3 font-normal">Severity</th>
              <th className="py-1.5 pr-3 font-normal">Package</th>
              <th className="py-1.5 pr-3 font-normal">Installed</th>
              <th className="py-1.5 pr-3 font-normal">Fixed in</th>
              <th className="py-1.5 font-normal">Title</th>
            </tr>
          </thead>
          <tbody>
            {shown.map((f) => (
              <tr key={`${f.vuln_id}-${f.pkg_name}-${f.installed_version}`} className="border-t border-border align-top">
                <td className="py-1.5 pr-3 font-ident whitespace-nowrap">
                  {f.url ? (
                    <a href={f.url} target="_blank" rel="noopener noreferrer" className="hover:underline">
                      {f.vuln_id}
                    </a>
                  ) : (
                    f.vuln_id
                  )}
                </td>
                <td className="py-1.5 pr-3">
                  <SeverityText severity={f.severity} />
                </td>
                <td className="py-1.5 pr-3 font-ident">{f.pkg_name}</td>
                <td className="py-1.5 pr-3 font-ident">{f.installed_version}</td>
                <td className="py-1.5 pr-3 font-ident">{f.fixed_version ?? <span className="text-meta">No fix</span>}</td>
                <td className="py-1.5 text-ink-muted">{f.title}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}

/** One digest: where it stands, what was found, and who runs it. */
export function DigestSection({ d, image, scanEnv }: { d: DigestDetail; image: string; scanEnv?: string }) {
  const canScan = useCanScan();
  const env = scanEnv ?? d.used_by.find((u) => canScan(u.env))?.env;
  const scan = useMutation({
    mutationFn: () => scanNow({ env: env!, images: [d.key] }),
    onSuccess: () => toast.success("Scan queued"),
    onError: toastError,
  });
  return (
    <Panel
      title={<span className="font-ident break-all">{d.key}</span>}
      actions={
        env &&
        canScan(env) && (
          <Button size="sm" variant="outline" disabled={scan.isPending} onClick={() => scan.mutate()}>
            <ScanSearch aria-hidden />
            Scan now
          </Button>
        )
      }
    >
      <div className="grid gap-4">
        <div className="flex flex-wrap items-center gap-x-5 gap-y-1 text-sm">
          <StateBadge state={STATE_LABEL[d.state]} tone={STATE_TONE[d.state]} />
          <span className="text-ink-muted">{d.last_scan_at ? `Scanned ${formatAgo(d.last_scan_at)}` : "No result yet"}</span>
          {d.trivy_image && <span className="font-ident text-xs text-meta">{d.trivy_image}</span>}
        </div>
        {d.error && <p className="rounded-md bg-raised p-3 font-ident text-xs text-fall whitespace-pre-wrap">{d.error}</p>}
        {d.counts ? (
          d.findings.length ? (
            <Findings findings={d.findings} />
          ) : (
            <p className="text-sm text-ink-muted">No known vulnerabilities in {image}.</p>
          )
        ) : null}
        <div className="grid gap-1.5">
          <div className="text-xs text-meta">Used by</div>
          <table aria-label="Used by" className="w-full text-sm">
            <tbody>
              {d.used_by.map((u, i) => (
                <tr key={i} className="border-t border-border">
                  <td className="py-1.5 pr-3">
                    <span className={`mr-2 inline-block size-2 rounded-full ${u.running ? "bg-live" : "bg-step"}`} aria-label={u.running ? "running" : "not running"} />
                    {u.env}
                    {u.node && <span className="text-meta">@{u.node}</span>}
                  </td>
                  <td className="py-1.5 pr-3 font-ident">{u.namespace}</td>
                  <td className="py-1.5 pr-3 font-ident">{u.workload}</td>
                  <td className="py-1.5 font-ident">{u.container || <span className="text-meta">no container</span>}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </div>
    </Panel>
  );
}

/** /security/image?image=…: one image across the fleet, per digest. */
export function SecurityImagePage() {
  useSecurityLive();
  const { image = "" } = useSearch({ strict: false }) as { image?: string };
  const q = useQuery(imageQuery(image));
  return (
    <Page title={<span className="font-ident">{image}</span>} kicker={<Link to={"/security" as never}>Security</Link>}>
      {q.error && <p className="text-sm text-fall">This image isn't in any environment you can see.</p>}
      <div className="grid gap-5">
        {q.data?.digests.map((d) => (
          <DigestSection key={d.key} d={d} image={image} />
        ))}
      </div>
    </Page>
  );
}
