import { JsonView } from "@/components/JsonView";
import { useNow, useNowRow } from "@/components/shell/now";
import { ObjectNow } from "@/components/shell/ObjectNow";
import { ChevronsDown, FilterX } from "lucide-react";
import { useInfiniteQuery, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useEffect, useMemo, useRef, useState } from "react";
import { auditQuery, envsQuery, keys, usersQuery, type AuditFilters } from "@/api/queries";
import type { AuditEntry } from "@/api/gen/types.gen";
import { columns, DataTable, type Column } from "@/components/DataTable";
import { Page } from "@/components/Page";
import { StateBadge } from "@/components/StateBadge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { NativeSelect } from "@/components/ui/native-select";
import { formatTime } from "@/lib/format";
import { SystemAvatar, UserAvatar } from "@/components/UserAvatar";
import { Slot } from "@/ee-slots";
import { useSubscription } from "@/live/LiveProvider";

type Row = AuditEntry & { envName: string };
const col = columns<Row>();

/** Where an entry's target lives in the UI, when it has a page there. */
function targetHref(e: Row): string | null {
  const id = e.target_id;
  if (!id) return null;
  const env = e.envName && encodeURIComponent(e.envName);
  const at = (path: string) => (env ? `/envs/${env}/${path}/${encodeURIComponent(id)}` : null);
  switch (e.target_type) {
    case "container":
      return at("containers");
    case "image":
      return at("images");
    case "volume":
      return at("volumes");
    case "network":
      return at("networks");
    case "stack":
      return at("stacks");
    case "user":
      return "/admin/users";
    case "env":
      return "/admin/envs";
    default:
      return null;
  }
}

const COLUMNS: Column<Row>[] = [
  col.accessor("ts", { header: "Time", cell: (c) => <span className="tabular-nums">{formatTime(c.getValue())}</span> }),
  col.accessor((e) => e.username ?? (e.user_id == null ? "system" : `user ${e.user_id}`), {
    id: "user",
    header: "User",
    cell: (c) => (
      <span className="flex items-center gap-2">
        {c.row.original.user_id == null ? <SystemAvatar /> : <UserAvatar name={c.getValue()} size="sm" />}
        {c.getValue()}
      </span>
    ),
  }),
  col.accessor("action", { header: "Action", cell: (c) => <code className="text-xs">{c.getValue()}</code> }),
  col.accessor("envName", { header: "Environment" }),
  col.accessor((e) => [e.target_type, e.target_id].filter(Boolean).join(" "), {
    id: "target",
    header: "Target",
    cell: (c) => {
      const href = targetHref(c.row.original);
      const text = <span className="block max-w-72 truncate">{c.getValue()}</span>;
      return href ? (
        <Link to={href as never} className="underline-offset-2 hover:underline">
          {text}
        </Link>
      ) : (
        text
      );
    },
  }),
  col.accessor("result", {
    header: "Result",
    cell: (c) => <StateBadge state={c.getValue()} tone={c.getValue() === "success" ? "run" : "fail"} />,
  }),
  col.accessor((e) => (e.detail == null ? "" : JSON.stringify(e.detail)), {
    id: "detail",
    header: "Detail",
    enableSorting: false,
    cell: (c) =>
      c.getValue() && (
        <details>
          <summary className="cursor-pointer text-muted-foreground">
            Details<span className="sr-only"> of {c.row.original.action}</span>
          </summary>
          <pre className="max-w-md font-mono text-xs whitespace-pre-wrap break-all">
            {JSON.stringify(c.row.original.detail, null, 2)}
          </pre>
        </details>
      ),
  }),
];

/** Time windows for the filter, in seconds back from now; 0 is everything. */
const WINDOWS: [number, string][] = [
  [0, "Any time"],
  [15 * 60, "Last 15 minutes"],
  [60 * 60, "Last hour"],
  [24 * 3600, "Last 24 hours"],
  [7 * 86400, "Last 7 days"],
];

/** A value that settles `ms` after it stops changing, for typed filters. */
function useSettled<T>(value: T, ms: number): T {
  const [settled, setSettled] = useState(value);
  useEffect(() => {
    const t = setTimeout(() => setSettled(value), ms);
    return () => clearTimeout(t);
  }, [value, ms]);
  return settled;
}

/**
 * Keeps the audit log current: every new entry refetches it, at most once a
 * second. Live is false while the channel is down, so the page can say so.
 */
function useLiveAudit(): boolean {
  const qc = useQueryClient();
  const [live, setLive] = useState(true);
  const pending = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => () => clearTimeout(pending.current ?? undefined), []);
  const refetch = () => {
    if (pending.current) return;
    pending.current = setTimeout(() => {
      pending.current = null;
      void qc.invalidateQueries({ queryKey: keys.audit });
    }, 1000);
  };
  useSubscription(
    { topic: "audit" },
    {
      onData: () => {
        setLive(true);
        refetch();
      },
      onResubscribe: () => {
        setLive(true);
        refetch();
      },
      onEnd: () => setLive(false),
      onError: () => setLive(false),
    },
  );
  return live;
}

/** Every mutating Docker and admin action, newest first. */
export function AuditPage() {
  const envs = useQuery(envsQuery);
  const users = useQuery(usersQuery);
  const [userId, setUserId] = useState("");
  const [envId, setEnvId] = useState("");
  const [action, setAction] = useState("");
  const [span, setSpan] = useState(0);
  const [failed, setFailed] = useState(false);
  const settledAction = useSettled(action.trim(), 300);
  // The window's start is fixed when it is picked, so the query key stays put.
  const [since, setSince] = useState<number | undefined>(undefined);
  const filters: AuditFilters = {
    ...(userId && { user_id: Number(userId) }),
    ...(envId && { env_id: Number(envId) }),
    ...(settledAction && { action: settledAction }),
    ...(since && { since }),
    ...(failed && { failed: true }),
  };
  const filtered = Object.keys(filters).length > 0;
  const audit = useInfiniteQuery(auditQuery(filters));
  const live = useLiveAudit();
  const rows = useMemo(() => {
    if (!audit.data) return undefined;
    const names = new Map(envs.data?.map((e) => [e.id, e.name]));
    return audit.data.pages.flat().map((e) => ({
      ...e,
      envName: e.environment_id == null ? "" : (names.get(e.environment_id) ?? `#${e.environment_id}`),
    }));
  }, [audit.data, envs.data]);
  const now = useNow();
  const active = useNowRow(rows, (e: AuditEntry) => String(e.id));
  const [cols] = useState(() => COLUMNS);
  const clear = () => {
    setUserId("");
    setEnvId("");
    setAction("");
    setSpan(0);
    setSince(undefined);
    setFailed(false);
  };
  return (
    <Page
      title="Audit log"
      meta={
        live ? (
          <span className="inline-flex items-center gap-1.5">
            <span aria-hidden className="size-1.5 rounded-full bg-live" />
            Live: new entries appear as they happen.
          </span>
        ) : (
          "Not live: the connection dropped. New entries show after a reload."
        )
      }
      actions={<Slot name="audit.toolbar" />}
    >
      <form
        role="search"
        aria-label="Filter the audit log"
        className="flex flex-wrap items-end gap-3"
        onSubmit={(e) => e.preventDefault()}
      >
        <div className="grid gap-1.5">
          <Label htmlFor="audit-user">User</Label>
          <NativeSelect id="audit-user" className="w-40" value={userId} onChange={(e) => setUserId(e.target.value)}>
            <option value="">Anyone</option>
            {users.data?.map((u) => (
              <option key={u.id} value={u.id}>
                {u.username}
              </option>
            ))}
          </NativeSelect>
        </div>
        <div className="grid gap-1.5">
          <Label htmlFor="audit-env">Environment</Label>
          <NativeSelect id="audit-env" className="w-40" value={envId} onChange={(e) => setEnvId(e.target.value)}>
            <option value="">Any</option>
            {envs.data?.map((e) => (
              <option key={e.id} value={e.id}>
                {e.name}
              </option>
            ))}
          </NativeSelect>
        </div>
        <div className="grid gap-1.5">
          <Label htmlFor="audit-action">Action</Label>
          <Input
            id="audit-action"
            className="w-48 font-mono text-xs"
            placeholder="container. or user.delete"
            aria-describedby="audit-action-hint"
            value={action}
            onChange={(e) => setAction(e.target.value)}
          />
          <span id="audit-action-hint" className="sr-only">
            An exact action, or a prefix ending in a dot
          </span>
        </div>
        <div className="grid gap-1.5">
          <Label htmlFor="audit-window">When</Label>
          <NativeSelect
            id="audit-window"
            className="w-40"
            value={span}
            onChange={(e) => {
              const w = Number(e.target.value);
              setSpan(w);
              setSince(w ? Math.floor(Date.now() / 1000) - w : undefined);
            }}
          >
            {WINDOWS.map(([w, label]) => (
              <option key={w} value={w}>
                {label}
              </option>
            ))}
          </NativeSelect>
        </div>
        <label className="flex h-9 items-center gap-2 text-sm">
          <Checkbox checked={failed} onCheckedChange={(v) => setFailed(v === true)} />
          Failures only
        </label>
        {filtered && (
          <Button type="button" variant="ghost" size="sm" className="h-9" onClick={clear}>
            <FilterX aria-hidden />
            Clear filters
          </Button>
        )}
      </form>
      {active && <AuditNow key={active.id} entry={active} />}
      <DataTable
        label="Audit log"
        data={rows}
        error={audit.error}
        columns={cols}
        onActivate={(r) => now.open(((e: AuditEntry) => String(e.id))(r))}
        activeId={active ? ((e: AuditEntry) => String(e.id))(active) : undefined}
        getRowId={(e) => String(e.id)}
        empty={filtered ? "No entries match these filters." : "Nothing has been audited yet."}
        phoneColumns={["ts", "user", "action"]}
      />
      {audit.hasNextPage && (
        <Button
          variant="outline"
          className="justify-self-start"
          disabled={audit.isFetchingNextPage}
          onClick={() => void audit.fetchNextPage()}
        >
          <ChevronsDown aria-hidden />
          Load older entries
        </Button>
      )}
    </Page>
  );
}

/** One audit record in the panel, whole: who, what, on which target, and the detail it carried. */
function AuditNow({ entry: e }: { entry: AuditEntry & { envName: string } }) {
  return (
    <ObjectNow
      title={e.action}
      subtitle={formatTime(e.ts)}
      facts={[
        { label: "User", value: e.username ?? "—" },
        { label: "Result", value: <span className={e.result === "ok" ? undefined : "text-fall"}>{e.result}</span> },
        { label: "Environment", value: e.envName || "—" },
      ]}
    >
      {e.target_type && (
        <p className="text-sm text-muted-foreground">
          {e.target_type} <span className="font-ident text-foreground">{e.target_id}</span>
        </p>
      )}
      {e.detail != null && <JsonView label="Detail" value={e.detail} />}
    </ObjectNow>
  );
}
