import { useQuery } from "@tanstack/react-query";
import { getRouteApi } from "@tanstack/react-router";
import type { ReactNode } from "react";
import { dfQuery, envStatusQuery, infoQuery, networksQuery, recentEventsQuery, volumesQuery } from "@/api/queries";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { summarizeDiskUsage, type DiskSummary, type Usage } from "@/lib/df";
import { errorMessage } from "@/lib/errors";
import { formatBytes, formatTime } from "@/lib/format";

const route = getRouteApi("/authed/shell/envs/$env/");

export function DashboardPage() {
  const { env } = route.useParams();
  const status = useQuery(envStatusQuery(env));
  const info = useQuery(infoQuery(env));
  const df = useQuery(dfQuery(env));
  const volumes = useQuery(volumesQuery(env));
  const networks = useQuery(networksQuery(env));
  const events = useQuery(recentEventsQuery(env));

  if (status.data?.state === "down") {
    return (
      <Page title={env}>
        <p role="alert" className="text-sm text-destructive">
          This environment is unavailable: {status.data.reason}
        </p>
      </Page>
    );
  }
  const i = info.data;
  const d = df.data;

  return (
    <Page title={env}>
      {info.error && (
        <p role="alert" className="text-sm text-destructive">
          {errorMessage(info.error)}
        </p>
      )}
      <div className="grid gap-4 sm:grid-cols-2 xl:grid-cols-4">
        <Stat title="Containers" value={i?.Containers}>
          {i && (
            <span>
              {i.ContainersRunning ?? 0} running · {i.ContainersPaused ?? 0} paused · {i.ContainersStopped ?? 0} stopped
            </span>
          )}
        </Stat>
        <Stat title="Images" value={i?.Images} />
        <Stat title="Volumes" value={volumes.data?.length} />
        <Stat title="Networks" value={networks.data?.length} />
      </div>
      <div className="grid gap-4 lg:grid-cols-2">
        <Card>
          <CardHeader>
            <CardTitle>Host</CardTitle>
          </CardHeader>
          <CardContent>
            <dl className="grid grid-cols-[auto_1fr] gap-x-6 gap-y-1 text-sm">
              <Row k="Engine" v={i?.ServerVersion} />
              <Row k="OS" v={i?.OperatingSystem} />
              <Row k="Architecture" v={i?.Architecture} />
              <Row k="CPUs" v={i?.NCPU} />
              <Row k="Memory" v={i ? formatBytes(i.MemTotal) : undefined} />
            </dl>
          </CardContent>
        </Card>
        <Card>
          <CardHeader>
            <CardTitle>Disk usage</CardTitle>
          </CardHeader>
          <CardContent>
            {d ? <DiskUsageList df={summarizeDiskUsage(d)} /> : <p className="text-sm text-muted-foreground">—</p>}
          </CardContent>
        </Card>
      </div>
      <Card>
        <CardHeader>
          <CardTitle>Recent events</CardTitle>
        </CardHeader>
        <CardContent>
          {events.data?.length ? (
            <ul className="grid gap-1 text-sm" aria-label="Recent events">
              {events.data.map((e, n) => (
                <li key={`${e.time}-${e.id}-${e.action}-${n}`} className="flex gap-3">
                  <span className="shrink-0 text-muted-foreground tabular-nums">{formatTime(e.time)}</span>
                  <span className="shrink-0 font-medium">
                    {e.kind} {e.action}
                  </span>
                  <span className="truncate text-muted-foreground">{e.name ?? e.id.slice(0, 12)}</span>
                </li>
              ))}
            </ul>
          ) : (
            <p className="text-sm text-muted-foreground">Events appear here as they happen.</p>
          )}
        </CardContent>
      </Card>
    </Page>
  );
}

function Page({ title, children }: { title: string; children: ReactNode }) {
  return (
    <div className="grid gap-4 p-6">
      <h1 className="text-2xl font-semibold">{title}</h1>
      {children}
    </div>
  );
}

function Stat({ title, value, children }: { title: string; value: number | undefined; children?: ReactNode }) {
  return (
    <Card className="gap-2">
      <CardHeader>
        <CardTitle className="text-sm font-medium text-muted-foreground">{title}</CardTitle>
      </CardHeader>
      <CardContent className="grid gap-1">
        <span className="text-3xl font-semibold tabular-nums" aria-label={title}>
          {value ?? "—"}
        </span>
        {children && <span className="text-xs text-muted-foreground">{children}</span>}
      </CardContent>
    </Card>
  );
}

function DiskUsageList({ df }: { df: DiskSummary }) {
  const rows: [string, Usage][] = [
    ["Images", df.images],
    ["Containers", df.containers],
    ["Volumes", df.volumes],
    ["Build cache", df.buildCache],
  ];
  return (
    <dl className="grid grid-cols-[auto_1fr] gap-x-6 gap-y-1 text-sm" aria-label="Disk usage">
      {rows.map(([k, u]) => (
        <Row
          key={k}
          k={k}
          v={u.reclaimable === undefined ? formatBytes(u.size) : `${formatBytes(u.size)} (${formatBytes(u.reclaimable)} reclaimable)`}
        />
      ))}
    </dl>
  );
}

function Row({ k, v }: { k: string; v: ReactNode }) {
  return (
    <>
      <dt className="text-muted-foreground">{k}</dt>
      <dd>{v ?? "—"}</dd>
    </>
  );
}
