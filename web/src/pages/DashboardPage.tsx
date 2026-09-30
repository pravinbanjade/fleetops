import { useQuery, type UseQueryResult } from "@tanstack/react-query";
import { getRouteApi, Link } from "@tanstack/react-router";
import { useId, type ReactNode } from "react";
import {
  containersQuery,
  dfQuery,
  ENV_DOWN_POLL_MS,
  type EngineInfo,
  envStatusQuery,
  infoQuery,
  networksQuery,
  RECENT_EVENTS_WINDOW,
  recentEventsQuery,
  volumesQuery,
} from "@/api/queries";
import type { ContainerSummary, DockerEvent } from "@/api/gen/types.gen";
import { useSession } from "@/auth/session";
import { Page } from "@/components/Page";
import { Sparkline } from "@/components/Sparkline";
import { StateBadge } from "@/components/StateBadge";
import { summarizeDiskUsage, type DiskSummary, type Usage } from "@/lib/df";
import { errorMessage } from "@/lib/errors";
import { formatBytes, formatPercent } from "@/lib/format";
import { attentionList, shownState, stateCounts } from "@/lib/health";
import { cn } from "@/lib/utils";
import { useCombinedStats, useLive } from "@/live/LiveProvider";

const route = getRouteApi("/authed/shell/envs/$env/");

/** Docker event actions that mean something stopped against its will. */
const BAD_ACTIONS = new Set(["die", "kill", "oom"]);

/**
 * An environment's home: what needs attention first, then the host's live
 * load, what just happened, and where the disk went. Everything on it
 * follows Docker events; when it can't, it says what it is showing instead.
 */
export function DashboardPage() {
  const { env } = route.useParams();
  const status = useQuery(envStatusQuery(env));
  const containers = useQuery(containersQuery(env));
  const info = useQuery(infoQuery(env));
  const df = useQuery(dfQuery(env));
  const volumes = useQuery(volumesQuery(env));
  const networks = useQuery(networksQuery(env));
  const events = useQuery(recentEventsQuery(env));
  const down = status.data?.state === "down" ? status.data.reason : null;
  // While the environment is down, a section is shown only for the data it
  // still has, not for the errors every query now returns.
  const shown = (q: UseQueryResult) => down === null || q.data !== undefined;
  const asOf = Math.max(containers.dataUpdatedAt, info.dataUpdatedAt);

  return (
    <Page title={env}>
      <Liveness down={down !== null} asOf={asOf} />
      {down !== null && <Unavailable reason={down} />}
      {shown(containers) && <StatusSection env={env} q={containers} />}
      {shown(info) && <HostSection env={env} info={info} containers={containers.data} />}
      <div className="grid gap-x-10 gap-y-8 border-t pt-6 lg:grid-cols-[minmax(0,3fr)_minmax(0,2fr)]">
        {shown(events) && <EventsSection env={env} q={events} />}
        {shown(df) && <DiskSection env={env} q={df} />}
      </div>
      <Inventory env={env} images={info.data?.Images} volumes={volumes.data?.length} networks={networks.data?.length} />
    </Page>
  );
}

/** Whether what's on screen is live, and if not, how old it is. */
function Liveness({ down, asOf }: { down: boolean; asOf: number }) {
  const { state } = useLive();
  const when = asOf ? `as of ${clock(asOf)}` : "";
  const [tone, text] = down
    ? ["bg-red-700", `Unavailable. Showing the last known state${when ? `, ${when}` : ""}.`]
    : state === "reconnecting"
      ? ["bg-amber-500", `Reconnecting. Showing data ${when || "loaded earlier"}.`]
      : state === "connecting"
        ? ["bg-muted-foreground", "Connecting…"]
        : ["bg-emerald-600", "Live. Updates as Docker reports changes."];
  return (
    <p data-testid="liveness" aria-live="polite" className="-mt-2 flex items-center gap-2 text-sm text-muted-foreground">
      <span aria-hidden className={cn("size-2 shrink-0 rounded-full", tone)} />
      {text}
    </p>
  );
}

function Unavailable({ reason }: { reason: string }) {
  const { user } = useSession();
  return (
    <div role="alert" className="grid gap-2 rounded-md border border-destructive/50 p-4">
      <p className="font-medium text-destructive">This environment is unavailable</p>
      <p className="font-mono text-xs break-all">{reason}</p>
      <p className="text-sm text-muted-foreground">
        Checking again every {ENV_DOWN_POLL_MS / 1000} seconds.{" "}
        {user.role === "admin" ? (
          <Link to="/admin/envs" className="text-foreground underline underline-offset-4">
            Manage environments
          </Link>
        ) : (
          "An administrator can change its connection settings."
        )}
      </p>
    </div>
  );
}

function StatusSection({ env, q }: { env: string; q: UseQueryResult<ContainerSummary[]> }) {
  const id = useId();
  if (q.isPending) return <Loading label="Loading containers" lines={2} />;
  if (!q.data) return <Section title="Containers">{<InlineError error={q.error} />}</Section>;
  const list = q.data;
  const attention = attentionList(list);
  const headline =
    list.length === 0
      ? "No containers yet"
      : attention.length === 0
        ? "Nothing needs attention"
        : `${attention.length} ${attention.length === 1 ? "container needs" : "containers need"} attention`;
  return (
    <section aria-labelledby={id} className="grid gap-3">
      <h2 id={id} className="text-lg font-semibold">
        {headline}
      </h2>
      {attention.length > 0 && (
        <ul aria-label="Needs attention" className="divide-y rounded-md border">
          {attention.map((c) => (
            <li key={c.id} className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1 px-3 py-2 text-sm">
              <StateBadge state={shownState(c)} />
              <Link
                to="/envs/$env/containers/$id"
                params={{ env, id: c.id }}
                search={{ tab: "logs" }}
                className="max-w-full truncate font-medium hover:underline"
              >
                {c.name}
              </Link>
              <span className="min-w-0 truncate text-muted-foreground">{c.status}</span>
            </li>
          ))}
        </ul>
      )}
      {list.length > 0 && (
        <ul aria-label="Containers by state" className="flex flex-wrap gap-2">
          {stateCounts(list).map(([state, n]) => (
            <li key={state}>
              <Link
                to="/envs/$env/containers"
                params={{ env }}
                search={{ q: state }}
                className="inline-flex items-center gap-1.5 rounded-md border px-2 py-1 text-sm hover:bg-accent"
              >
                <span className="font-medium tabular-nums">{n}</span> <StateBadge state={state} />
              </Link>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

function HostSection({
  env,
  info,
  containers,
}: {
  env: string;
  info: UseQueryResult<EngineInfo>;
  containers: ContainerSummary[] | undefined;
}) {
  const running = (containers ?? []).filter((c) => c.state === "running").map((c) => c.id);
  const usage = useCombinedStats(env, running);
  const i = info.data;
  const cpus = i?.NCPU ?? 0;
  // Docker reports CPU as a percentage of one core; the host has `cpus`.
  const cpuSeries = cpus ? usage.cpu.map((v) => v / cpus) : [];
  const facts = [i?.ServerVersion && `Engine ${i.ServerVersion}`, i?.OperatingSystem, i?.Architecture].filter(Boolean);
  return (
    <Section title="Host" aside={facts.join(" · ")} className="border-t pt-6">
      {info.error && !i ? (
        <InlineError error={info.error} />
      ) : running.length === 0 ? (
        <p className="text-sm text-muted-foreground">No containers are running.</p>
      ) : (
        <>
          <div className="grid gap-6 sm:grid-cols-2">
            <Readout
              label="CPU"
              value={cpuSeries.length ? formatPercent(cpuSeries.at(-1)!) : "—"}
              detail={cpus ? `of ${cpus} ${cpus === 1 ? "CPU" : "CPUs"}` : ""}
              values={cpuSeries}
              max={Math.max(5, ...cpuSeries) * 1.25}
            />
            <Readout
              label="Memory"
              value={usage.memory.length ? formatBytes(usage.memory.at(-1)!) : "—"}
              detail={i?.MemTotal ? `of ${formatBytes(i.MemTotal)}` : ""}
              values={usage.memory}
              max={Math.max(1, ...usage.memory) * 1.25}
            />
          </div>
          <p className="text-xs text-muted-foreground">
            Used by the {running.length} running {running.length === 1 ? "container" : "containers"}, over the last
            minute.
          </p>
        </>
      )}
    </Section>
  );
}

/** A live value with its last minute; the line's scale follows the minute's peak. */
function Readout({
  label,
  value,
  detail,
  values,
  max,
}: {
  label: string;
  value: string;
  detail: string;
  values: number[];
  max: number;
}) {
  return (
    <div role="group" aria-label={label} className="grid gap-1">
      <div className="text-sm text-muted-foreground">{label}</div>
      <div className="flex items-end justify-between gap-4">
        <p className="tabular-nums">
          <span className="text-2xl font-semibold">{value}</span>{" "}
          <span className="text-sm whitespace-nowrap text-muted-foreground">{detail}</span>
        </p>
        <Sparkline values={values} max={max} label={`${label} over the last minute`} width={160} height={32} />
      </div>
    </div>
  );
}

function EventsSection({ env, q }: { env: string; q: UseQueryResult<DockerEvent[]> }) {
  return (
    <Section title="Recent events" aside={`Last ${RECENT_EVENTS_WINDOW / 60} minutes`}>
      {q.isPending ? (
        <Loading label="Loading recent events" lines={4} />
      ) : q.error ? (
        <InlineError error={q.error} prefix="Couldn't load earlier events" />
      ) : q.data.length === 0 ? (
        <p className="text-sm text-muted-foreground">Nothing has happened in the last 15 minutes.</p>
      ) : (
        <ul aria-label="Recent events" className="grid max-h-96 gap-1 overflow-auto text-sm">
          {q.data.map((e) => (
            <li key={`${e.time}-${e.kind}-${e.id}-${e.action}`} className="flex min-w-0 flex-wrap gap-x-3">
              <span className="shrink-0 text-muted-foreground tabular-nums">{clock(e.time * 1000)}</span>
              <span className={cn("shrink-0 font-medium", BAD_ACTIONS.has(e.action) && "text-destructive")}>
                {e.kind} {e.action}
              </span>
              <span className="min-w-0 truncate text-muted-foreground">
                <EventSubject env={env} e={e} />
                {e.attributes.exitCode !== undefined && ` · exit code ${e.attributes.exitCode}`}
              </span>
            </li>
          ))}
        </ul>
      )}
    </Section>
  );
}

function EventSubject({ env, e }: { env: string; e: DockerEvent }) {
  const name = e.name ?? e.id.slice(0, 12);
  if (e.kind !== "container" || e.action === "destroy") return <>{name}</>;
  return (
    <Link to="/envs/$env/containers/$id" params={{ env, id: e.id }} className="text-foreground hover:underline">
      {name}
    </Link>
  );
}

const DISK: { key: keyof DiskSummary; label: string; to?: "/envs/$env/images" | "/envs/$env/containers" | "/envs/$env/volumes" }[] = [
  { key: "images", label: "Images", to: "/envs/$env/images" },
  { key: "containers", label: "Containers", to: "/envs/$env/containers" },
  { key: "volumes", label: "Volumes", to: "/envs/$env/volumes" },
  { key: "buildCache", label: "Build cache" },
];

function DiskSection({ env, q }: { env: string; q: UseQueryResult<unknown> }) {
  const df = q.data ? summarizeDiskUsage(q.data as Parameters<typeof summarizeDiskUsage>[0]) : null;
  const total = df ? DISK.reduce((n, d) => n + df[d.key].size, 0) : 0;
  return (
    <Section title="Disk usage" aside={df ? formatBytes(total) : undefined}>
      {q.isPending ? (
        <Loading label="Loading disk usage" lines={4} />
      ) : !df ? (
        <InlineError error={q.error} />
      ) : (
        <ul aria-label="Disk usage" className="grid gap-3 text-sm">
          {DISK.map((d) => (
            <DiskRow key={d.key} env={env} label={d.label} to={d.to} usage={df[d.key]} total={total} />
          ))}
        </ul>
      )}
    </Section>
  );
}

function DiskRow({
  env,
  label,
  to,
  usage,
  total,
}: {
  env: string;
  label: string;
  to?: (typeof DISK)[number]["to"];
  usage: Usage;
  total: number;
}) {
  const share = total > 0 ? usage.size / total : 0;
  return (
    <li aria-label={label} className="grid gap-1">
      <div className="flex items-baseline justify-between gap-3">
        {to ? (
          <Link to={to} params={{ env }} className="font-medium hover:underline">
            {label}
          </Link>
        ) : (
          <span className="font-medium">{label}</span>
        )}
        <span className="whitespace-nowrap tabular-nums">{formatBytes(usage.size)}</span>
      </div>
      <div aria-hidden className="h-1 overflow-hidden rounded-full bg-muted">
        <div className="h-full rounded-full bg-muted-foreground" style={{ width: `${(share * 100).toFixed(1)}%` }} />
      </div>
      {usage.reclaimable !== undefined && usage.reclaimable > 0 && (
        <span className="text-xs text-muted-foreground tabular-nums">{formatBytes(usage.reclaimable)} reclaimable</span>
      )}
    </li>
  );
}

function Inventory({
  env,
  images,
  volumes,
  networks,
}: {
  env: string;
  images: number | undefined;
  volumes: number | undefined;
  networks: number | undefined;
}) {
  const items: [number | undefined, string, "/envs/$env/images" | "/envs/$env/volumes" | "/envs/$env/networks"][] = [
    [images, "image", "/envs/$env/images"],
    [volumes, "volume", "/envs/$env/volumes"],
    [networks, "network", "/envs/$env/networks"],
  ];
  return (
    <nav aria-label="Other resources" className="flex flex-wrap gap-x-6 gap-y-1 border-t pt-4 text-sm">
      {items.map(([n, noun, to]) => (
        <Link key={noun} to={to} params={{ env }} className="text-muted-foreground hover:text-foreground hover:underline">
          <span className="tabular-nums">{n ?? "—"}</span> {n === 1 ? noun : `${noun}s`}
        </Link>
      ))}
    </nav>
  );
}

function Section({
  title,
  aside,
  className,
  children,
}: {
  title: string;
  aside?: string;
  className?: string;
  children: ReactNode;
}) {
  const id = useId();
  return (
    <section aria-labelledby={id} className={cn("grid content-start gap-3", className)}>
      <div className="flex flex-wrap items-baseline justify-between gap-x-4 gap-y-1">
        <h2 id={id} className="text-base font-semibold">
          {title}
        </h2>
        {aside && <span className="text-sm text-muted-foreground">{aside}</span>}
      </div>
      {children}
    </section>
  );
}

function InlineError({ error, prefix }: { error: unknown; prefix?: string }) {
  return (
    <p role="alert" className="text-sm text-destructive">
      {prefix ? `${prefix}: ${errorMessage(error)}` : errorMessage(error)}
    </p>
  );
}

/** A placeholder while a section's data loads; it doesn't pretend to be "—". */
function Loading({ label, lines }: { label: string; lines: number }) {
  return (
    <div role="status" className="grid gap-2">
      <span className="sr-only">{label}…</span>
      {Array.from({ length: lines }, (_, n) => (
        <div
          key={n}
          aria-hidden
          className="h-4 rounded bg-muted motion-safe:animate-pulse"
          style={{ width: `${60 + ((n * 37) % 35)}%` }}
        />
      ))}
    </div>
  );
}

/** A time of day to the second, in the viewer's locale. */
function clock(ms: number): string {
  return new Date(ms).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" });
}
