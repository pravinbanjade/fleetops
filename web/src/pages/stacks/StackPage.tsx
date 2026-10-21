import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, getRouteApi } from "@tanstack/react-router";
import { Play, RotateCw, Square, Trash2 } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import { stackAction } from "@/api/gen/sdk.gen";
import type { DeploySource, StackActionName } from "@/api/gen/types.gen";
import { useEnvAccess } from "@/api/access";
import { containersQuery, keys, stackQuery } from "@/api/queries";
import { Page } from "@/components/Page";
import { ContainerState, StateBadge } from "@/components/StateBadge";
import { Button } from "@/components/ui/button";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { errorMessage } from "@/lib/errors";
import { GitTab, secretKey } from "./GitTab";
import { RemoveStackDialog } from "./RemoveStackDialog";
import { ReviewDeployDialog } from "./ReviewDeployDialog";
import { RevisionsTab } from "./RevisionsTab";
import { StackEditor } from "./StackEditor";

const route = getRouteApi("/authed/shell/envs/$env/stacks/$stack");

const ICON = { start: Play, stop: Square, restart: RotateCw } as const;
const DONE: Record<StackActionName, string> = { start: "Started", stop: "Stopped", restart: "Restarted" };

/** One stack: its draft, its containers and its revisions. */
export function StackPage() {
  const { env, stack } = route.useParams();
  const qc = useQueryClient();
  const detail = useQuery(stackQuery(env, stack));
  const containers = useQuery(containersQuery(env));
  const { can } = useEnvAccess(env);
  const [review, setReview] = useState<DeploySource | null>(null);
  const [removing, setRemoving] = useState(false);
  // A stack just created from git opens on its Git tab, to show the secret.
  const [tab, setTab] = useState(() => (qc.getQueryData(secretKey(env, stack)) ? "git" : "editor"));
  const act = useMutation({
    mutationFn: async (action: StackActionName) => {
      const { data } = await stackAction({ path: { env, stack, action } });
      await qc.invalidateQueries({ queryKey: keys.containers(env) });
      const msg = `${DONE[action]} ${data.affected} container${data.affected === 1 ? "" : "s"} of ${stack}`;
      if (data.errors.length) toast.error(`${msg}. ${data.errors.join("; ")}`);
      else toast.success(msg);
    },
  });
  if (detail.error)
    return (
      <Page title={stack}>
        <p role="alert" className="text-sm text-destructive">
          {errorMessage(detail.error)}
        </p>
      </Page>
    );
  if (!detail.data)
    return (
      <Page title={stack}>
        <p className="text-sm text-muted-foreground">Loading…</p>
      </Page>
    );
  const d = detail.data;
  const members = (containers.data ?? []).filter((c) => c.compose_project === stack && !c.name.includes("-fleetops-old-"));
  const byService = Object.entries(
    members.reduce<Record<string, typeof members>>((acc, c) => {
      (acc[c.compose_service ?? "?"] ??= []).push(c);
      return acc;
    }, {}),
  ).sort(([a], [b]) => a.localeCompare(b));
  const canOperate = can("stacks.operate");
  const isGit = d.stack.source === "git";
  return (
    <Page
      title={stack}
      meta={
        <span className="flex items-center gap-2">
          {d.deployed ? (
            <>
              Revision #{d.deployed.number} <StateBadge state={d.deployed.status} />
            </>
          ) : (
            "Not deployed yet"
          )}
        </span>
      }
      actions={
        <>
          {canOperate &&
            (["start", "stop", "restart"] as const).map((a) => {
              const Icon = ICON[a];
              return (
                <Button key={a} variant="outline" size="sm" disabled={act.isPending} onClick={() => act.mutate(a)}>
                  <Icon />
                  {a[0].toUpperCase() + a.slice(1)}
                </Button>
              );
            })}
          {can("stacks.delete") && (
            <Button
              variant="outline"
              size="sm"
              className="text-destructive hover:border-destructive/50 hover:bg-destructive/10 hover:text-destructive"
              onClick={() => setRemoving(true)}
            >
              <Trash2 />
              Remove
            </Button>
          )}
        </>
      }
    >
      <Tabs value={tab} onValueChange={setTab}>
        <TabsList>
          <TabsTrigger value="editor">Editor</TabsTrigger>
          {isGit && <TabsTrigger value="git">Git</TabsTrigger>}
          <TabsTrigger value="services">Services</TabsTrigger>
          <TabsTrigger value="revisions">Revisions</TabsTrigger>
        </TabsList>
        <TabsContent value="editor" className="pt-4">
          <StackEditor
            key={d.draft.updated_at}
            env={env}
            detail={d}
            canEdit={can("stacks.create")}
            canDeploy={canOperate}
            onReview={() => setReview({ source: "draft" })}
          />
        </TabsContent>
        {isGit && (
          <TabsContent value="git" className="pt-4">
            <GitTab
              env={env}
              stack={stack}
              canEdit={can("stacks.create")}
              canOperate={canOperate}
              onReview={setReview}
            />
          </TabsContent>
        )}
        <TabsContent value="services" className="pt-4">
          {byService.length === 0 ? (
            <p className="text-sm text-muted-foreground">No containers.</p>
          ) : (
            <div className="grid gap-4">
              {byService.map(([svc, cs]) => (
                <section key={svc} aria-label={`Service ${svc}`} className="grid gap-1">
                  <h2 className="font-ident text-sm font-semibold">{svc}</h2>
                  <ul className="font-strip overflow-hidden rounded-md border">
                    {cs.map((c) => (
                      <li
                        key={c.id}
                        className="grid grid-cols-[auto_minmax(0,1fr)] items-center gap-x-3 border-b px-3 py-2 last:border-b-0 sm:grid-cols-[auto_minmax(0,18rem)_minmax(0,1fr)]"
                      >
                        <ContainerState container={c} />
                        <Link
                          to="/envs/$env/containers/$id"
                          params={{ env, id: c.id }}
                          className="truncate font-ident font-medium hover:underline"
                        >
                          {c.name}
                        </Link>
                        <span className="col-start-2 truncate text-sm text-muted-foreground sm:col-start-auto">{c.status}</span>
                      </li>
                    ))}
                  </ul>
                </section>
              ))}
            </div>
          )}
        </TabsContent>
        <TabsContent value="revisions" className="pt-4">
          <RevisionsTab
            env={env}
            stack={stack}
            deployed={d.stack.deployed_revision}
            canRollback={canOperate}
            onRollback={(revision) => setReview({ source: "revision", revision })}
          />
        </TabsContent>
      </Tabs>
      {review && <ReviewDeployDialog env={env} stack={stack} source={review} onClose={() => setReview(null)} />}
      {removing && <RemoveStackDialog env={env} stack={stack} onClose={() => setRemoving(false)} />}
    </Page>
  );
}
