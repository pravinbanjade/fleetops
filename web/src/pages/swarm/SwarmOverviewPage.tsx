import { swarmStatus } from "@/lib/findings";
import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useState } from "react";
import type { DockerEvent } from "@/api/gen/types.gen";
import { envStatusQuery, swarmKeys, swarmNodesQuery, swarmServicesQuery } from "@/api/queries";
import { useSession } from "@/auth/session";
import { Page } from "@/components/Page";
import { formatAgo } from "@/lib/format";
import { useSwarmEvents } from "@/live/swarmEvents";
import { useSwarmTasks } from "@/live/swarmTasks";
import { TrustCard } from "./NodesPage";
import { FeedPaused } from "./TasksPage";
import { failed, taskName } from "./shared";

/** How far back a failed task still needs attention. */
const RECENT_SECS = 3600;

/** A swarm's home: what needs attention first, then recent events and the serving manager. */
export function SwarmOverviewPage({ env }: { env: string }) {
  const { user } = useSession();
  useSwarmEvents(env);
  const status = useQuery(envStatusQuery(env, true));
  const nodes = useQuery(swarmNodesQuery(env));
  const services = useQuery(swarmServicesQuery(env));
  const { tasks, paused } = useSwarmTasks(env);
  const events = useQuery<DockerEvent[]>({ queryKey: swarmKeys.events(env), queryFn: () => [], staleTime: Infinity });
  // Read once per mount: "the last hour" needs no ticking clock.
  const [now] = useState(() => Date.now() / 1000);
  // The feed is live; the service list's counts change only with its events.
  const runningOf = (id: string) => tasks && tasks.filter((t) => t.service_id === id && t.state === "running").length;
  const hostOf = (id?: string | null) => nodes.data?.find((n) => n.id === id)?.hostname ?? id ?? "";
  const items: { key: string; text: string; to?: string }[] = [
    ...(nodes.data ?? []).filter((n) => n.state !== "ready").map((n) => ({ key: `n-${n.id}`, text: `${n.hostname} is ${n.state}` })),
    ...(services.data ?? [])
      .map((s) => ({ s, running: runningOf(s.id) ?? s.tasks_running }))
      .filter(({ s, running }) => running < s.tasks_desired)
      .map(({ s, running }) => ({ key: `s-${s.id}`, text: `${s.name} has ${running} of ${s.tasks_desired} tasks running`, to: s.id })),
    ...(tasks ?? [])
      .filter((t) => failed(t) && now - t.updated_at < RECENT_SECS)
      .map((t) => ({ key: `t-${t.id}`, text: `${taskName(t)} ${t.state} on ${hostOf(t.node_id)}: ${t.error ?? t.message}`, to: t.service_id })),
  ];
  const v = status.data?.state === "up" && status.data.version.kind === "swarm" ? status.data.version : null;
  return (
    <Page
      title={env}
      kicker="Swarm"
      meta={[nodes.data && swarmStatus(nodes.data), v && `${v.managers} managers, ${v.workers} workers · Docker ${v.engine}`].filter(Boolean).join(" · ")}
    >
      <TrustCard env={env} admin={user.role === "admin"} />
      <FeedPaused reason={paused} />
      {status.data?.state === "down" && (
        <p role="status" className="mb-3 rounded-md border border-destructive/40 bg-destructive/5 p-3 text-sm">
          {status.data.reason}
        </p>
      )}
      <div className="grid gap-4 lg:grid-cols-2">
        <section aria-label="Needs attention" className="grid content-start gap-2 rounded-lg border p-4">
          <h2 className="text-sm font-semibold">Needs attention</h2>
          {items.length === 0 ? (
            <p className="text-sm text-muted-foreground">Nothing needs attention.</p>
          ) : (
            <ul className="grid gap-1.5 text-sm">
              {items.map((i) => (
                <li key={i.key}>
                  {i.to ? (
                    <Link to="/envs/$env/swarm/services/$id" params={{ env, id: i.to }} className="hover:underline">
                      {i.text}
                    </Link>
                  ) : (
                    i.text
                  )}
                </li>
              ))}
            </ul>
          )}
        </section>
        <section aria-label="Recent events" className="grid content-start gap-2 rounded-lg border p-4">
          <h2 className="text-sm font-semibold">Recent events</h2>
          {(events.data ?? []).length === 0 ? (
            <p className="text-sm text-muted-foreground">No events since this page opened.</p>
          ) : (
            <ul className="grid gap-1 text-sm">
              {events.data!.map((e, i) => (
                <li key={`${e.time}-${i}`} className="flex gap-2">
                  <span className="w-20 shrink-0 text-muted-foreground">{formatAgo(e.time)}</span>
                  <span>
                    {e.kind} {e.name ?? e.id.slice(0, 12)} {e.action}
                  </span>
                </li>
              ))}
            </ul>
          )}
        </section>
      </div>
      {v && <p className="mt-4 text-sm text-muted-foreground">Serving manager: {hostOf(v.serving_node)}</p>}
    </Page>
  );
}
