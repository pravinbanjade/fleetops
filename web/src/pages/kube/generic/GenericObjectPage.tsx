import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, getRouteApi, useNavigate } from "@tanstack/react-router";
import { FileText, Info, ListTree } from "lucide-react";
import { lazy, Suspense, useMemo, useState, type ReactNode } from "react";
import { deleteGenericKube } from "@/api/gen/sdk.gen";
import { useEnvAccess } from "@/api/access";
import { genericEventsQuery, genericObjectQuery, keys } from "@/api/queries";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { DetailError, Details } from "@/components/Details";
import { Page } from "@/components/Page";
import { Button } from "@/components/ui/button";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { formatAgo } from "@/lib/format";
import { useKubeWatch } from "@/live/kubeWatch";
import { KubeHealth } from "../KubeHealth";
import { LivePaused } from "../LivePaused";
import { Conditions, EventStrips } from "../ObjectParts";
import type { KubeTab } from "../tabs";
import { printed } from "./GenericListPage";
import { refKind, type Ref } from "./ref";
import { useDiscoveredKind } from "./useDiscoveredKind";

const YamlDialog = lazy(() => import("@/components/yaml-editor/YamlDialog"));
const route = getRouteApi("/authed/shell/envs/$env/k8s/r/$group/$version/$resource/$ns/$name");

/** One object of a discovered kind: summary, conditions, YAML and events. */
export function GenericObjectPage() {
  const { env, group, version, resource, ns, name } = route.useParams();
  const { tab = "summary" } = route.useSearch();
  const r = useMemo<Ref>(() => ({ group, version, resource }), [group, version, resource]);
  const navigate = useNavigate();
  const qc = useQueryClient();
  const { canWrite } = useEnvAccess(env);
  const { kind: k } = useDiscoveredKind(env, r);
  const obj = useQuery(genericObjectQuery(env, r, ns, name));
  const events = useQuery({ ...genericEventsQuery(env, r, ns, name), enabled: tab === "events" });
  const [open, setOpen] = useState<"edit" | "delete" | null>(null);
  const [gone, setGone] = useState(false);
  const live = useKubeWatch(env, refKind(r), ns === "-" ? undefined : ns, (ev) => {
    if (ev.type === "snapshot") setGone(!ev.rows.some((x) => x.name === name));
    if ((ev.type === "added" || ev.type === "modified") && ev.row.name === name) {
      setGone(false);
      void qc.invalidateQueries({ queryKey: keys.kubeObject(env, refKind(r), ns, name) });
    }
    if (ev.type === "deleted" && ev.name === name) setGone(true);
  });
  const label = k?.kind ?? resource;
  const back = (
    <Link to="/envs/$env/k8s/r/$group/$version/$resource" params={{ env, ...r }} className="text-sm underline">
      Back to {label}
    </Link>
  );
  if (obj.error) return <DetailError error={obj.error} what={label.toLowerCase()} back={back} />;
  const o = obj.data;
  const canEdit = canWrite && !!k?.verbs.includes("update");
  const canDelete = canWrite && !!k?.verbs.includes("delete");
  const actions =
    canEdit || canDelete ? (
      <div className="flex flex-wrap gap-2">
        {canEdit && (
          <Button variant="outline" size="sm" onClick={() => setOpen("edit")}>
            Edit YAML
          </Button>
        )}
        {canDelete && (
          <Button variant="outline" size="sm" className="text-state-fail-text hover:text-state-fail-text" onClick={() => setOpen("delete")}>
            Delete
          </Button>
        )}
      </div>
    ) : undefined;
  const summary: [string, ReactNode][] = o
    ? [
        ["Kind", label],
        ["API", <span className="font-ident">{`${group}/${version}`}</span>],
        ["Namespace", o.row.namespace ? <span className="font-ident">{o.row.namespace}</span> : undefined],
        ["Age", formatAgo(o.row.created)],
        ...(k?.columns ?? []).map((p): [string, ReactNode] => [p.name, printed(p.type, o.row.columns[p.name]) || "—"]),
      ]
    : [];
  return (
    <Page title={name} meta={o && <KubeHealth health={o.row.health} label={o.row.status || "—"} />} actions={actions}>
      {live.paused && <LivePaused paused={live.paused} onRetry={live.retry} />}
      {gone && (
        <p role="status" className="mb-3 rounded-md border border-state-fail/40 bg-state-fail/10 px-3 py-2 text-sm">
          This object was deleted. {back}
        </p>
      )}
      {open === "edit" && k && (
        <Suspense fallback={null}>
          <YamlDialog env={env} target={{ mode: "edit-generic", ref: r, kind: k.kind, ns, name }} onClose={() => setOpen(null)} />
        </Suspense>
      )}
      {open === "delete" && (
        <ConfirmDialog
          open
          onOpenChange={(v) => !v && setOpen(null)}
          title={`Delete ${name}?`}
          description="This cannot be undone."
          confirmLabel="Delete"
          destructive
          onConfirm={async () => {
            await deleteGenericKube({ path: { env, ...r, ns, name }, throwOnError: true });
            void navigate({ to: "/envs/$env/k8s/r/$group/$version/$resource", params: { env, ...r } });
          }}
        />
      )}
      <Tabs value={tab} onValueChange={(t) => void navigate({ to: ".", search: { tab: t as KubeTab }, replace: true })}>
        <TabsList>
          <TabsTrigger value="summary">
            <Info aria-hidden strokeWidth={1.75} />
            Summary
          </TabsTrigger>
          <TabsTrigger value="yaml">
            <FileText aria-hidden strokeWidth={1.75} />
            YAML
          </TabsTrigger>
          <TabsTrigger value="events">
            <ListTree aria-hidden strokeWidth={1.75} />
            Events
          </TabsTrigger>
        </TabsList>
        <TabsContent value="summary">
          {o && (
            <div className="grid gap-4">
              <Details label="Summary" items={summary} />
              {o.owners.length > 0 && (
                <p className="text-sm">
                  Owned by{" "}
                  {o.owners.map((x, i) => (
                    <span key={`${x.kind}/${x.name}`}>
                      {i > 0 && " → "}
                      {x.link ? (
                        <Link to="/envs/$env/k8s/$kind/$ns/$name" params={{ env, kind: x.link, ns, name: x.name }} className="hover:underline">
                          {x.name}
                        </Link>
                      ) : (
                        <span>{x.name}</span>
                      )}{" "}
                      <span className="text-muted-foreground">({x.kind})</span>
                    </span>
                  ))}
                </p>
              )}
              {o.conditions.length > 0 && <Conditions conditions={o.conditions} />}
            </div>
          )}
        </TabsContent>
        <TabsContent value="yaml">
          {o && (
            <pre aria-label="Object YAML" className="overflow-x-auto rounded-md border bg-muted/40 p-3 font-mono text-xs">
              {o.yaml}
            </pre>
          )}
        </TabsContent>
        <TabsContent value="events">
          <EventStrips events={events.data} pending={events.isPending} error={events.error} />
        </TabsContent>
      </Tabs>
    </Page>
  );
}
