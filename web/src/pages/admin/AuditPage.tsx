import { useInfiniteQuery, useQuery } from "@tanstack/react-query";
import { useMemo, useState } from "react";
import { auditQuery, envsQuery } from "@/api/queries";
import type { AuditEntry } from "@/api/gen/types.gen";
import { columns, DataTable, type Column } from "@/components/DataTable";
import { Page } from "@/components/Page";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { formatTime } from "@/lib/format";
import { SystemAvatar, UserAvatar } from "@/components/UserAvatar";
import { Slot } from "@/ee-slots";

type Row = AuditEntry & { envName: string };
const col = columns<Row>();

const COLUMNS: Column<Row>[] = [
  col.accessor("ts", { header: "Time", cell: (c) => formatTime(c.getValue()) }),
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
    cell: (c) => <span className="block max-w-72 truncate">{c.getValue()}</span>,
  }),
  col.accessor("result", {
    header: "Result",
    cell: (c) =>
      c.getValue() === "success" ? (
        <Badge variant="secondary">success</Badge>
      ) : (
        <Badge variant="destructive">{c.getValue()}</Badge>
      ),
  }),
  col.accessor((e) => (e.detail == null ? "" : JSON.stringify(e.detail)), {
    id: "detail",
    header: "Detail",
    enableSorting: false,
    cell: (c) =>
      c.getValue() && (
        <details>
          <summary className="cursor-pointer text-muted-foreground">show</summary>
          <pre className="max-w-md font-mono text-xs whitespace-pre-wrap break-all">
            {JSON.stringify(c.row.original.detail, null, 2)}
          </pre>
        </details>
      ),
  }),
];

/** Every mutating Docker and admin action, newest first. */
export function AuditPage() {
  const audit = useInfiniteQuery(auditQuery);
  const envs = useQuery(envsQuery);
  const rows = useMemo(() => {
    if (!audit.data) return undefined;
    const names = new Map(envs.data?.map((e) => [e.id, e.name]));
    return audit.data.pages.flat().map((e) => ({
      ...e,
      envName: e.environment_id == null ? "" : (names.get(e.environment_id) ?? `#${e.environment_id}`),
    }));
  }, [audit.data, envs.data]);
  const [cols] = useState(() => COLUMNS);
  return (
    <Page
      title="Audit log"
      actions={
        <>
          <Slot name="audit.toolbar" />
          <Button variant="outline" disabled={audit.isFetching} onClick={() => void audit.refetch()}>
            Refresh
          </Button>
        </>
      }
    >
      <DataTable
        label="Audit log"
        data={rows}
        error={audit.error}
        columns={cols}
        getRowId={(e) => String(e.id)}
        empty="Nothing has been audited yet."
      />
      {audit.hasNextPage && (
        <Button
          variant="outline"
          className="justify-self-start"
          disabled={audit.isFetchingNextPage}
          onClick={() => void audit.fetchNextPage()}
        >
          Load older entries
        </Button>
      )}
    </Page>
  );
}
