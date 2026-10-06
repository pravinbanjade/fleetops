import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, getRouteApi, useNavigate } from "@tanstack/react-router";
import { FileText, Info, ListTree, ScrollText, TerminalSquare } from "lucide-react";
import { Fragment, lazy, Suspense, useState } from "react";
import { useEnvAccess } from "@/api/access";
import { keys, kubeEventsQuery, kubeObjectQuery } from "@/api/queries";
import { useKubeWatch } from "@/live/kubeWatch";
import { DetailError } from "@/components/Details";
import { Page } from "@/components/Page";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { formatAgo } from "@/lib/format";
import { KubeActions } from "./KubeActions";
import { KubeHealth } from "./KubeHealth";
import { asKind, CLUSTER_SCOPED, KIND_LABEL } from "./kinds";
import { LivePaused } from "./LivePaused";
import type { KubeTab } from "./tabs";

const LogsTab = lazy(() => import("@/pages/containers/logs/LogsTab").then((m) => ({ default: m.LogsTab })));
const TerminalTab = lazy(() => import("@/pages/containers/terminal/TerminalTab").then((m) => ({ default: m.TerminalTab })));
const route = getRouteApi("/authed/shell/envs/$env/k8s/$kind/$ns/$name");

export function KubeObjectPage() {
  const { env, kind: raw, ns, name } = route.useParams();
  const { tab = "summary" } = route.useSearch();
  const navigate = useNavigate();
  const kind = asKind(raw) ?? "pods";
  const { canWrite } = useEnvAccess(env);
  const obj = useQuery(kubeObjectQuery(env, kind, ns, name));
  const events = useQuery({ ...kubeEventsQuery(env, kind, ns, name), enabled: tab === "events" });
  const [container, setContainer] = useState<string | undefined>();
  const qc = useQueryClient();
  const [gone, setGone] = useState(false);
  const live = useKubeWatch(env, kind, CLUSTER_SCOPED.includes(kind) ? undefined : ns, (ev) => {
    if (ev.type === "snapshot") setGone(!ev.rows.some((r) => r.name === name));
    if ((ev.type === "added" || ev.type === "modified") && ev.row.name === name) {
      setGone(false);
      // The events key extends this one, so an open Events tab refetches too.
      void qc.invalidateQueries({ queryKey: keys.kubeObject(env, kind, ns, name) });
    }
    if (ev.type === "deleted" && ev.name === name) setGone(true);
  });
  if (obj.error)
    return (
      <DetailError
        error={obj.error}
        what={KIND_LABEL[kind].replace(/s$/, "").toLowerCase()}
        back={
          <Link to="/envs/$env/k8s/$kind" params={{ env, kind }} className="text-sm underline">
            Back to {KIND_LABEL[kind]}
          </Link>
        }
      />
    );
  const o = obj.data;
  const isPod = kind === "pods";
  const chosen = container ?? o?.containers[0];
  const replicas = Number(o?.row.columns.ready?.split("/")[1] ?? 1);
  const tabs: [KubeTab, string, typeof Info][] = [
    ["summary", "Summary", Info],
    ["yaml", "YAML", FileText],
    ["events", "Events", ListTree],
    ...(isPod ? ([["logs", "Logs", ScrollText]] as [KubeTab, string, typeof Info][]) : []),
    ...(isPod && canWrite ? ([["terminal", "Terminal", TerminalSquare]] as [KubeTab, string, typeof Info][]) : []),
  ];
  return (
    <Page title={name} meta={o && <KubeHealth health={o.row.health} label={o.row.status} />} actions={o && <KubeActions env={env} kind={kind} ns={ns} name={name} replicas={replicas} />}>
      {live.paused && <LivePaused paused={live.paused} onRetry={live.retry} />}
      {gone && (
        <p role="status" className="mb-3 rounded-md border border-state-fail/40 bg-state-fail/10 px-3 py-2 text-sm">
          This object was deleted.{" "}
          <Link to="/envs/$env/k8s/$kind" params={{ env, kind }} className="underline">
            Back to {KIND_LABEL[kind]}
          </Link>
        </p>
      )}
      {o && (
        <div className="mb-3">
      {o.owners.length > 0 && (
        <p className="text-sm">
          Owned by{" "}
          {o.owners.map((r, i) => (
            <span key={`${r.kind}/${r.name}`}>
              {i > 0 && " → "}
              {r.link ? (
                <Link to="/envs/$env/k8s/$kind/$ns/$name" params={{ env, kind: r.link, ns, name: r.name }} className="hover:underline">{r.name}</Link>
              ) : (
                <span>{r.name}</span>
              )}{" "}
              <span className="text-muted-foreground">({r.kind})</span>
            </span>
          ))}
        </p>
      )}
        </div>
      )}
      <Tabs value={tab} onValueChange={(t) => void navigate({ to: ".", search: { tab: t as KubeTab }, replace: true })}>
        <TabsList>
          {tabs.map(([value, label, Icon]) => (
            <TabsTrigger key={value} value={value}>
              <Icon aria-hidden strokeWidth={1.75} />
              {label}
            </TabsTrigger>
          ))}
        </TabsList>
        <TabsContent value="summary">
          {o && (
            <div className="grid gap-4">
              <dl className="grid grid-cols-[10rem_1fr] gap-y-1 text-sm">
                <dt className="text-muted-foreground">Kind</dt><dd>{KIND_LABEL[kind].replace(/s$/, "")}</dd>
                {o.row.namespace && (<><dt className="text-muted-foreground">Namespace</dt><dd>{o.row.namespace}</dd></>)}
                <dt className="text-muted-foreground">Age</dt><dd>{formatAgo(o.row.created)}</dd>
                {Object.entries(o.row.columns).map(([k, v]) => (
                  <Fragment key={k}>
                    <dt className="text-muted-foreground">{k.replace(/_/g, " ")}</dt>
                    <dd>{v || "—"}</dd>
                  </Fragment>
                ))}
              </dl>
              {o.conditions.length > 0 && (
                <table className="text-sm" aria-label="Conditions">
                  <thead><tr><th className="text-left">Condition</th><th className="text-left">Status</th><th className="text-left">Reason</th><th className="text-left">Message</th></tr></thead>
                  <tbody>{o.conditions.map((c) => (<tr key={c.type}><td>{c.type}</td><td>{c.status}</td><td>{c.reason}</td><td>{c.message}</td></tr>))}</tbody>
                </table>
              )}
            </div>
          )}
        </TabsContent>
        <TabsContent value="yaml">
          {o && <pre aria-label="Object YAML" className="overflow-x-auto rounded-md border bg-muted/40 p-3 font-mono text-xs">{o.yaml}</pre>}
        </TabsContent>
        <TabsContent value="events">
          <ul className="grid gap-2 text-sm">
            {(events.data ?? []).map((e, i) => (
              <li key={i} className={e.type === "Warning" ? "text-state-fail" : undefined}>
                <span className="font-medium">{e.reason}</span> {e.message}{" "}
                <span className="text-muted-foreground">{e.last_seen ? formatAgo(e.last_seen) : ""} ×{e.count}</span>
              </li>
            ))}
            {events.data?.length === 0 && <li className="text-muted-foreground">No recent events.</li>}
          </ul>
        </TabsContent>
        {isPod && o && (
          <>
            <ContainerPicker containers={o.containers} value={chosen} onChange={setContainer} hidden={tab !== "logs" && tab !== "terminal"} />
            <TabsContent value="logs">
              <Suspense fallback={null}>
                <LogsTab key={chosen} env={env} id={`${ns}/${name}`} name={name} running={o.row.status === "Running"} container={chosen} />
              </Suspense>
            </TabsContent>
            {canWrite && (
              <TabsContent value="terminal">
                <Suspense fallback={null}>
                  <TerminalTab key={chosen} env={env} id={`${ns}/${name}`} running={o.row.status === "Running"} container={chosen} />
                </Suspense>
              </TabsContent>
            )}
          </>
        )}
      </Tabs>
    </Page>
  );
}

function ContainerPicker({ containers, value, onChange, hidden }: { containers: string[]; value?: string; onChange: (c: string) => void; hidden: boolean }) {
  if (hidden || containers.length < 2) return null;
  return (
    <label className="my-2 flex items-center gap-2 text-sm">
      <span className="text-muted-foreground">Container</span>
      <select aria-label="Container" className="h-8 rounded-md border bg-background px-2" value={value} onChange={(e) => onChange(e.target.value)}>
        {containers.map((c) => (<option key={c} value={c}>{c}</option>))}
      </select>
    </label>
  );
}
