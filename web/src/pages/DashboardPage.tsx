import { useQuery, type UseQueryResult } from "@tanstack/react-query";
import { getRouteApi, Link } from "@tanstack/react-router";
import {
  CircleCheck,
  Cpu,
  HardDrive,
  History,
  Layers,
  MemoryStick,
  ScrollText,
  Server,
  TriangleAlert,
  Waypoints,
  type LucideIcon,
} from "lucide-react";
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
import type { ContainerSummary, DockerEvent, NetworkSummary } from "@/api/gen/types.gen";
import { useSession } from "@/auth/session";
import { ImageRef } from "@/components/ImageRef";
import { Page } from "@/components/Page";
import { Readout } from "@/components/Readout";
import { ContainerState, StateBadge, stateTone } from "@/components/StateBadge";
import { Usage } from "@/components/Usage";
import { walk } from "@/components/walk";
import { summarizeDiskUsage, type DiskSummary, type Usage as DiskUse } from "@/lib/df";
import { errorMessage } from "@/lib/errors";
import { formatBytes, formatPercent } from "@/lib/format";
import { attentionList, stateCounts } from "@/lib/health";
import { cn } from "@/lib/utils";
import { useCombinedStats, useLive } from "@/live/LiveProvider";

const route = getRouteApi("/authed/shell/envs/$env/");

/** Docker event actions that mean something stopped against its will. */
const BAD_ACTIONS = new Set(["die", "kill", "oom"]);

/** Running strips shown on the board before "all containers" takes over. */
const RUNNING_SHOWN = 8;

/**
 * An environment's home, laid out as a strip board: what needs attention
 * first, then what is running, what just happened, and on the right the
 * host's live load and where the disk went. Everything on it follows Docker
 * events; when it can't, it says what it is showing instead.
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
    <Page
      title={env}
      meta={
        <span className="flex flex-wrap items-center gap-x-4 gap-y-1">
          <Liveness down={down !== null} asOf={asOf} />
          {info.data && <HostFacts info={info.data} />}
        </span>
      }
      actions={shown(info) && <HostSection env={env} info={info} containers={containers.data} />}
    >
      {down !== null && <Unavailable reason={down} />}
      {/* Two columns on one grid: the board on the left (what needs attention,
          what runs, what just happened) and disk usage on the right. Both
          start on the same line; each ends where its content does. */}
      <div className="grid gap-5 xl:grid-cols-[minmax(0,1fr)_24rem]">
        <div className="grid min-w-0 content-start gap-5">
          {shown(containers) && <StatusSection env={env} q={containers} />}
          {containers.data && <RunningBay env={env} list={containers.data} />}
          {shown(events) && <EventsSection env={env} q={events} />}
        </div>
        <div className="grid min-w-0 content-start gap-5">
          {shown(df) && (
            <DiskSection env={env} q={df}>
              <Inventory
                env={env}
                images={info.data?.Images}
                volumes={volumes.data?.length}
                networks={networks.data?.length}
              />
            </DiskSection>
          )}
          {shown(networks) && <NetworksSection env={env} q={networks} />}
        </div>
      </div>
    </Page>
  );
}

/** Whether what's on screen is live, and if not, how old it is. */
function Liveness({ down, asOf }: { down: boolean; asOf: number }) {
  const { state } = useLive();
  const when = asOf ? `as of ${clock(asOf)}` : "";
  const [tone, text] = down
    ? ["bg-state-fail", `Unavailable. Showing the last known state${when ? `, ${when}` : ""}.`]
    : state === "reconnecting"
      ? ["bg-state-wait", `Reconnecting. Showing data ${when || "loaded earlier"}.`]
      : state === "connecting"
        ? ["bg-muted-foreground", "Connecting…"]
        : ["bg-live", "Live. Updates as Docker reports changes."];
  return (
    <p data-testid="liveness" aria-live="polite" className="flex items-center gap-2">
      <span aria-hidden className={cn("size-2 shrink-0 rounded-full", tone)} />
      {text}
    </p>
  );
}

function Unavailable({ reason }: { reason: string }) {
  const { user } = useSession();
  return (
    <div role="alert" className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1.5 rounded-lg border border-destructive/50 bg-destructive/8 p-4">
      <TriangleAlert aria-hidden className="row-span-3 mt-0.5 size-5 text-destructive" />
      <p className="font-semibold text-destructive">This environment is unavailable</p>
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

/** The tones a shown state's share of the bar is drawn in. */
const TONE_FILL = { run: "bg-state-run", wait: "bg-state-wait", fail: "bg-state-fail", idle: "bg-input" } as const;

function StatusSection({ env, q }: { env: string; q: UseQueryResult<ContainerSummary[]> }) {
  const id = useId();
  if (q.isPending) return <Loading label="Loading containers" lines={3} />;
  if (!q.data) return <Section title="Containers">{<InlineError error={q.error} />}</Section>;
  const list = q.data;
  const attention = attentionList(list);
  const counts = stateCounts(list);
  const headline =
    list.length === 0
      ? "No containers yet"
      : attention.length === 0
        ? "Nothing needs attention"
        : `${attention.length} ${attention.length === 1 ? "container needs" : "containers need"} attention`;
  const Icon = attention.length ? TriangleAlert : CircleCheck;
  // One panel: the headline answers the first question, the strips name the
  // containers behind it, and the footer counts the whole environment.
  return (
    <section aria-labelledby={id} className="overflow-hidden rounded-lg border bg-card">
      <div className="flex flex-wrap items-center gap-x-2.5 gap-y-1 border-b px-4 py-2.5">
        {list.length > 0 && (
          <Icon aria-hidden className={cn("size-4.5", attention.length ? "text-state-fail" : "text-state-run")} />
        )}
        <h2 id={id} className="font-callsign text-base font-semibold">
          {headline}
        </h2>
        {attention.length > 1 && (
          <span className="ml-auto hidden text-xs text-muted-foreground sm:inline">
            <kbd className="font-mono">j</kbd> <kbd className="font-mono">k</kbd> to move,{" "}
            <kbd className="font-mono">↵</kbd> opens logs
          </span>
        )}
      </div>
      {attention.length > 0 && (
        <ul aria-label="Needs attention" onKeyDown={walk} className="border-b">
          {attention.map((c) => (
            <li
              key={c.id}
              className="font-strip grid min-w-0 grid-cols-[auto_minmax(0,1fr)] items-center gap-x-3 gap-y-0.5 border-b px-4 py-2.5 text-sm last:border-b-0 has-[a:focus-visible]:bg-cord-soft sm:grid-cols-[auto_minmax(0,16rem)_minmax(0,1fr)_auto]"
            >
              <ContainerState container={c} />
              <Link
                data-strip
                to="/envs/$env/containers/$id"
                params={{ env, id: c.id }}
                search={{ tab: "logs" }}
                className="truncate font-ident font-medium hover:underline focus-visible:outline-none"
              >
                {c.name}
              </Link>
              <span className="col-start-2 min-w-0 truncate text-muted-foreground sm:col-start-auto">{c.status}</span>
              <span aria-hidden className="hidden items-center gap-1 text-xs text-muted-foreground sm:flex">
                <ScrollText className="size-3.5" />
                Logs
              </span>
            </li>
          ))}
        </ul>
      )}
      {list.length > 0 && (
        <div className="grid gap-2.5 bg-muted/25 px-4 py-3">
          <div aria-hidden className="flex h-1.5 gap-px overflow-hidden rounded-full bg-muted">
            {counts.map(([state, n]) => (
              <span
                key={state}
                className={TONE_FILL[stateTone(state)]}
                style={{ width: `${((n / list.length) * 100).toFixed(2)}%` }}
              />
            ))}
          </div>
          <ul aria-label="Containers by state" className="-mx-1.5 flex flex-wrap gap-1">
            {counts.map(([state, n]) => (
              <li key={state}>
                <Link
                  to="/envs/$env/containers"
                  params={{ env }}
                  search={{ q: state }}
                  className="inline-flex h-8 items-center gap-2 rounded-md px-1.5 text-sm transition-colors hover:bg-accent"
                >
                  <span className="font-callsign text-base font-semibold tabular-nums">{n}</span> <StateBadge state={state} />
                </Link>
              </li>
            ))}
          </ul>
        </div>
      )}
      {list.length === 0 && <p className="px-4 py-3 text-sm text-muted-foreground">Containers appear here as soon as Docker reports them.</p>}
    </section>
  );
}

/** The running containers as live strips: the busiest part of the board. */
function RunningBay({ env, list }: { env: string; list: ContainerSummary[] }) {
  const running = list
    .filter((c) => c.state === "running")
    .sort((a, b) => a.name.localeCompare(b.name));
  if (running.length === 0) return null;
  const shownList = running.slice(0, RUNNING_SHOWN);
  return (
    <Section
      title="Running"
      icon={Server}
      aside={
        running.length > RUNNING_SHOWN ? (
          <Link to="/envs/$env/containers" params={{ env }} search={{ q: "running" }} className="hover:text-foreground hover:underline">
            All {running.length} running
          </Link>
        ) : (
          `${running.length}`
        )
      }
      flush
    >
      {/* The strip drops fields by the bay's own width, not the window's. */}
      <ul aria-label="Running containers" onKeyDown={walk} className="font-strip @container text-sm">
        {shownList.map((c) => (
          <li
            key={c.id}
            className="grid grid-cols-[minmax(0,1fr)_auto] items-center gap-x-4 gap-y-1 border-b px-4 py-2 last:border-b-0 has-[a:focus-visible]:bg-cord-soft @md:grid-cols-[minmax(0,1fr)_8.5rem_9.5rem] @xl:grid-cols-[auto_minmax(0,1fr)_8.5rem_9.5rem] @3xl:grid-cols-[auto_minmax(10rem,18rem)_minmax(0,1fr)_8.5rem_9.5rem]"
          >
            <ContainerState container={c} className="hidden @xl:inline-flex" />
            <Link
              data-strip
              to="/envs/$env/containers/$id"
              params={{ env, id: c.id }}
              className="truncate font-ident font-medium hover:underline focus-visible:outline-none"
            >
              {c.name}
            </Link>
            <span className="hidden truncate @3xl:block">
              <ImageRef value={c.image} />
            </span>
            <Usage env={env} container={c} kind="cpu" />
            <span className="hidden @md:block">
              <Usage env={env} container={c} kind="memory" />
            </span>
          </li>
        ))}
      </ul>
    </Section>
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
  const mem = usage.memory.at(-1);
  return (
    <section aria-label="Host" className="grid w-full min-w-0 grid-cols-2 gap-2 sm:w-auto xl:w-[24rem]">
      {info.error && !i ? (
        <InlineError error={info.error} />
      ) : running.length === 0 ? (
        <p className="text-sm text-muted-foreground">No containers are running.</p>
      ) : (
        <>
          <Readout
            label="CPU"
            icon={Cpu}
            value={cpuSeries.length ? formatPercent(cpuSeries.at(-1)!) : "—"}
            detail={cpus ? `of ${cpus} ${cpus === 1 ? "CPU" : "CPUs"}` : ""}
            share={cpuSeries.length ? cpuSeries.at(-1)! / 100 : undefined}
            values={cpuSeries}
            max={Math.max(5, ...cpuSeries) * 1.25}
          />
          <Readout
            label="Memory"
            icon={MemoryStick}
            value={mem !== undefined ? formatBytes(mem) : "—"}
            detail={i?.MemTotal ? `of ${formatBytes(i.MemTotal)}` : ""}
            share={mem !== undefined && i?.MemTotal ? mem / i.MemTotal : undefined}
            values={usage.memory}
            max={i?.MemTotal || Math.max(1, ...usage.memory) * 1.25}
          />
          <p className="sr-only">
            Used by the {running.length} running {running.length === 1 ? "container" : "containers"}, over the last
            minute.
          </p>
        </>
      )}
    </section>
  );
}

/** The engine's version, OS and architecture, beside the liveness line. */
function HostFacts({ info }: { info: EngineInfo }) {
  const facts = [info.ServerVersion && `Engine ${info.ServerVersion}`, info.OperatingSystem, info.Architecture].filter(Boolean);
  if (facts.length === 0) return null;
  return <span className="text-xs">{facts.join(" · ")}</span>;
}

function EventsSection({ env, q }: { env: string; q: UseQueryResult<DockerEvent[]> }) {
  return (
    <Section
      title="Recent events"
      icon={History}
      aside={`Last ${RECENT_EVENTS_WINDOW / 60} minutes`}
      flush
    >
      {q.isPending ? (
        <div className="p-4">
          <Loading label="Loading recent events" lines={4} />
        </div>
      ) : q.error ? (
        <div className="p-4">
          <InlineError error={q.error} prefix="Couldn't load earlier events" />
        </div>
      ) : q.data.length === 0 ? (
        <p className="p-4 text-sm text-muted-foreground">Nothing has happened in the last 15 minutes.</p>
      ) : (
        <ul
          aria-label="Recent events"
          tabIndex={0}
          className="font-strip max-h-96 overflow-auto pb-8 text-sm [mask-image:linear-gradient(to_bottom,black_calc(100%-2.5rem),transparent)] focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring"
        >
          {q.data.map((e) => (
            <li
              key={`${e.time}-${e.kind}-${e.id}-${e.action}`}
              className="grid min-w-0 grid-cols-[auto_minmax(7rem,auto)_minmax(0,1fr)] items-baseline gap-x-3 border-b px-4 py-1.5 last:border-b-0"
            >
              <time className="font-mono text-xs whitespace-nowrap text-muted-foreground">{clock(e.time * 1000)}</time>
              <span className={cn("truncate font-medium", BAD_ACTIONS.has(e.action) && "text-destructive")}>
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
  // Container and image IDs are shortened; a volume or network is named by its ID.
  const name = e.name ?? (/^(sha256:)?[0-9a-f]{64}$/.test(e.id) ? e.id.replace(/^sha256:/, "").slice(0, 12) : e.id);
  if (e.kind !== "container" || e.action === "destroy") return <span className="font-ident">{name}</span>;
  return (
    <Link to="/envs/$env/containers/$id" params={{ env, id: e.id }} className="font-ident text-foreground hover:underline">
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

function DiskSection({ env, q, children }: { env: string; q: UseQueryResult<unknown>; children?: ReactNode }) {
  const df = q.data ? summarizeDiskUsage(q.data as Parameters<typeof summarizeDiskUsage>[0]) : null;
  const total = df ? DISK.reduce((n, d) => n + df[d.key].size, 0) : 0;
  return (
    <Section title="Disk usage" icon={HardDrive} aside={df ? formatBytes(total) : undefined}>
      {q.isPending ? (
        <Loading label="Loading disk usage" lines={4} />
      ) : !df ? (
        <InlineError error={q.error} />
      ) : (
        <ul aria-label="Disk usage" className="grid gap-x-6 gap-y-4 text-sm sm:grid-cols-2 lg:grid-cols-4 xl:grid-cols-1">
          {DISK.map((d) => (
            <DiskRow key={d.key} env={env} label={d.label} to={d.to} usage={df[d.key]} total={total} />
          ))}
        </ul>
      )}
      {children}
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
  usage: DiskUse;
  total: number;
}) {
  const share = total > 0 ? usage.size / total : 0;
  return (
    <li aria-label={label} className="grid gap-1.5">
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
        <div className="h-full rounded-full bg-foreground/45" style={{ width: `${(share * 100).toFixed(1)}%` }} />
      </div>
      {usage.reclaimable !== undefined && usage.reclaimable > 0 && (
        <span className="text-xs text-muted-foreground tabular-nums">{formatBytes(usage.reclaimable)} reclaimable</span>
      )}
    </li>
  );
}

/** Networks shown on the board before "all networks" takes over. */
const NETWORKS_SHOWN = 6;
/** Docker's own networks, listed after the ones someone made. */
const BUILT_IN = new Set(["bridge", "host", "none"]);

/** The environment's networks: name, driver and subnet, the user's own first. */
function NetworksSection({ env, q }: { env: string; q: UseQueryResult<NetworkSummary[]> }) {
  const list = [...(q.data ?? [])].sort(
    (a, b) => Number(BUILT_IN.has(a.name)) - Number(BUILT_IN.has(b.name)) || a.name.localeCompare(b.name),
  );
  const more = list.length > NETWORKS_SHOWN;
  return (
    <Section
      title="Networks"
      icon={Waypoints}
      aside={
        more ? (
          <Link to="/envs/$env/networks" params={{ env }} className="hover:text-foreground hover:underline">
            All {list.length}
          </Link>
        ) : q.data ? (
          `${list.length}`
        ) : undefined
      }
      flush
    >
      {q.isPending ? (
        <div className="p-4">
          <Loading label="Loading networks" lines={3} />
        </div>
      ) : q.error ? (
        <div className="p-4">
          <InlineError error={q.error} />
        </div>
      ) : list.length === 0 ? (
        <p className="p-4 text-sm text-muted-foreground">No networks.</p>
      ) : (
        <ul aria-label="Networks" className="font-strip text-sm">
          {list.slice(0, NETWORKS_SHOWN).map((n) => (
            <li key={n.id} className="grid grid-cols-[minmax(0,1fr)_auto] items-baseline gap-x-3 border-b px-4 py-2 last:border-b-0">
              <Link
                to="/envs/$env/networks/$id"
                params={{ env, id: n.id }}
                className={cn("truncate font-ident font-medium hover:underline", BUILT_IN.has(n.name) && "text-muted-foreground")}
              >
                {n.name}
              </Link>
              <span className="text-xs text-muted-foreground">
                {n.driver}
                {n.internal && " · internal"}
              </span>
              {n.subnet && <code className="col-span-2 truncate text-xs text-muted-foreground">{n.subnet}</code>}
            </li>
          ))}
        </ul>
      )}
    </Section>
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
  const items: [number | undefined, string, "/envs/$env/images" | "/envs/$env/volumes" | "/envs/$env/networks", LucideIcon][] = [
    [images, "image", "/envs/$env/images", Layers],
    [volumes, "volume", "/envs/$env/volumes", HardDrive],
    [networks, "network", "/envs/$env/networks", Waypoints],
  ];
  return (
    <nav aria-label="Other resources" className="-mx-4 mt-4 -mb-4 grid grid-cols-3 border-t text-sm">
      {items.map(([n, noun, to, Icon]) => (
        <Link
          key={noun}
          to={to}
          params={{ env }}
          className="flex items-center justify-center gap-2 border-l px-2 py-2.5 text-muted-foreground transition-colors first:border-l-0 hover:bg-accent hover:text-foreground"
        >
          <Icon aria-hidden className="size-4" strokeWidth={1.75} />
          <span>
            <span className="font-medium text-foreground tabular-nums">{n ?? "—"}</span> {n === 1 ? noun : `${noun}s`}
          </span>
        </Link>
      ))}
    </nav>
  );
}

/**
 * A panel of the board: a titled strip header over its content. `flush`
 * lets strip lists run edge to edge.
 */
function Section({
  title,
  icon: Icon,
  aside,
  flush,
  children,
}: {
  title: string;
  icon?: LucideIcon;
  aside?: ReactNode;
  flush?: boolean;
  children: ReactNode;
}) {
  const id = useId();
  return (
    <section aria-labelledby={id} className="grid content-start overflow-hidden rounded-lg border bg-card">
      <div className="flex flex-wrap items-center justify-between gap-x-4 gap-y-1 border-b px-4 py-2.5">
        <h2 id={id} className="flex items-center gap-2 text-sm font-semibold">
          {Icon && <Icon aria-hidden className="size-4 text-muted-foreground" strokeWidth={1.75} />}
          {title}
        </h2>
        {aside && <span className="text-xs text-muted-foreground">{aside}</span>}
      </div>
      <div className={flush ? undefined : "p-4"}>{children}</div>
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
          className="h-4 rounded-xs bg-muted motion-safe:animate-pulse"
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
