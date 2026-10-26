import { useQuery } from "@tanstack/react-query";
import { getRouteApi, useNavigate } from "@tanstack/react-router";
import { lazy, Suspense, useState } from "react";
import { useEnvAccess } from "@/api/access";
import { helmCapabilitiesQuery, helmHistoryQuery, helmReleaseQuery } from "@/api/queries";
import { Page } from "@/components/Page";
import { Button } from "@/components/ui/button";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { errorMessage } from "@/lib/errors";
import { formatAgo } from "@/lib/format";
import { HelmStatus } from "./HelmStatus";
import type { HelmAction } from "./HelmPlanDialog";
import type { HelmTab } from "./tabs";

const route = getRouteApi("/authed/shell/envs/$env/k8s/helm/$ns/$name");
const HelmPlanDialog = lazy(() => import("./HelmPlanDialog"));
const HelmDeployDialog = lazy(() => import("./HelmDeployDialog"));

const TABS: [HelmTab, string][] = [
  ["overview", "Overview"],
  ["history", "History"],
  ["values", "Values"],
  ["manifest", "Manifest"],
  ["notes", "Notes"],
];

function Text({ label, text, empty }: { label: string; text: string; empty: string }) {
  return text ? (
    <pre aria-label={label} className="max-h-[70vh] overflow-auto rounded-md bg-[#0a0a0a] p-3 font-mono text-xs">
      {text}
    </pre>
  ) : (
    <p className="text-sm text-muted-foreground">{empty}</p>
  );
}

export function ReleasePage() {
  const { env, ns, name } = route.useParams();
  const { tab = "overview" } = route.useSearch();
  const navigate = useNavigate();
  const { can } = useEnvAccess(env);
  const detail = useQuery(helmReleaseQuery(env, ns, name));
  const history = useQuery({ ...helmHistoryQuery(env, ns, name), enabled: tab === "history" });
  const [action, setAction] = useState<HelmAction | null>(null);
  const caps = useQuery(helmCapabilitiesQuery(env));
  const [upgrading, setUpgrading] = useState(false);
  const r = detail.data?.release;
  const hidden = detail.data?.hidden === true;
  const HIDDEN = "Values are hidden: your role lacks Reveal on Helm releases.";
  return (
    <Page
      kicker="Helm release"
      title={name}
      meta={
        r && (
          <span className="flex items-center gap-2 text-sm text-muted-foreground">
            <HelmStatus status={r.status} error={r.error} />
            {r.chart}-{r.chart_version} · revision {r.revision} · {ns}
          </span>
        )
      }
      actions={
        r &&
        (can("helm.operate", ns) || can("helm.delete", ns)) && (
          <span className="flex items-center gap-2">
            {/* Upgrade starts from the current values, so it needs to see them. */}
            {caps.data?.install && can("helm.operate", ns) && !hidden && (
              <Button variant="outline" size="sm" onClick={() => setUpgrading(true)}>
                Upgrade
              </Button>
            )}
            {can("helm.delete", ns) && (
              <Button variant="outline" size="sm" onClick={() => setAction({ kind: "uninstall" })}>
                Uninstall
              </Button>
            )}
          </span>
        )
      }
    >
      {upgrading && (
        <Suspense fallback={null}>
          <HelmDeployDialog
            env={env}
            target={{ kind: "upgrade", ns, name, source: detail.data?.chart_source ?? null, values: detail.data?.values ?? "" }}
            onClose={() => setUpgrading(false)}
          />
        </Suspense>
      )}
      {action && (
        <Suspense fallback={null}>
          <HelmPlanDialog env={env} ns={ns} name={name} action={action} onClose={() => setAction(null)} />
        </Suspense>
      )}
      {detail.isError && (
        <p role="alert" className="text-sm text-state-fail">
          {errorMessage(detail.error)}
        </p>
      )}
      <Tabs value={tab} onValueChange={(t) => void navigate({ to: ".", search: { tab: t as HelmTab }, replace: true })}>
        <TabsList>
          {TABS.map(([value, label]) => (
            <TabsTrigger key={value} value={value}>
              {label}
            </TabsTrigger>
          ))}
        </TabsList>
        <TabsContent value="overview">
          {r && (
            <dl className="grid grid-cols-[max-content_1fr] gap-x-6 gap-y-1 text-sm">
              <dt className="text-muted-foreground">Chart</dt>
              <dd>
                {r.chart} {r.chart_version}
              </dd>
              <dt className="text-muted-foreground">App version</dt>
              <dd>{r.app_version}</dd>
              <dt className="text-muted-foreground">Status</dt>
              <dd>
                <HelmStatus status={r.status} error={r.error} />
              </dd>
              <dt className="text-muted-foreground">Description</dt>
              <dd>{detail.data?.description}</dd>
              <dt className="text-muted-foreground">Last deployed</dt>
              <dd>{r.updated ? formatAgo(r.updated) : ""}</dd>
            </dl>
          )}
        </TabsContent>
        <TabsContent value="history">
          <table aria-label="Revisions" className="w-full text-sm">
            <thead className="text-left text-muted-foreground">
              <tr>
                <th>Revision</th>
                <th>Status</th>
                <th>Chart</th>
                <th>App</th>
                <th>Description</th>
                <th>Updated</th>
                <th>
                  <span className="sr-only">Actions</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {history.data
                ?.slice()
                .reverse()
                .map((h) => (
                  <tr key={h.revision} className="border-t">
                    <td>{h.revision}</td>
                    <td>
                      <HelmStatus status={h.status} />
                    </td>
                    <td>{h.chart_version}</td>
                    <td>{h.app_version}</td>
                    <td>{h.description}</td>
                    <td>{h.updated ? formatAgo(h.updated) : ""}</td>
                    <td className="text-right">
                      {can("helm.operate", ns) && r && h.revision !== r.revision && h.status !== "unreadable" && (
                        <Button variant="outline" size="sm" onClick={() => setAction({ kind: "rollback", revision: h.revision })}>
                          Roll back to {h.revision}
                        </Button>
                      )}
                    </td>
                  </tr>
                ))}
            </tbody>
          </table>
        </TabsContent>
        <TabsContent value="values">
          <p className="mb-2 text-xs text-muted-foreground">Keys that look like credentials are masked.</p>
          <Text label="Values" text={detail.data?.values ?? ""} empty={hidden ? HIDDEN : "No values were supplied."} />
        </TabsContent>
        <TabsContent value="manifest">
          <Text label="Manifest" text={detail.data?.manifest ?? ""} empty="The manifest is empty." />
        </TabsContent>
        <TabsContent value="notes">
          <Text label="Notes" text={detail.data?.notes ?? ""} empty={hidden ? HIDDEN : "This chart has no notes."} />
        </TabsContent>
      </Tabs>
    </Page>
  );
}
