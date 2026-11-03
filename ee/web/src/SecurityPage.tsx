import { useMutation, useQuery } from "@tanstack/react-query";
import { Link, useNavigate, useSearch } from "@tanstack/react-router";
import { ScanSearch } from "lucide-react";
import { toast } from "sonner";
import type { ImageRow, SecuritySummary } from "@/api/gen/types.gen";
import { envsQuery, meQuery } from "@/api/queries";
import { columns, DataTable, type Column } from "@/components/DataTable";
import { Page } from "@/components/Page";
import { StateBadge } from "@/components/StateBadge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { NativeSelect } from "@/components/ui/native-select";
import { toastError } from "@/lib/errors";
import { useCanScan } from "./SecurityImagePage";
import { imagesQuery, scanNow, securitySearch, settingsQuery, STATE_LABEL, STATE_TONE, useSecurityLive, type ImagesFilter, type SecuritySearch } from "./security";

const col = columns<ImageRow>();

/** A count that means something only with a result: a dash otherwise, never a clean 0. */
function Count({ row, n, tone }: { row: ImageRow; n: number; tone?: string }) {
  if (!row.has_result) return <span className="text-meta">—</span>;
  return <span className={n > 0 ? tone : "text-ink-muted"}>{n}</span>;
}

function rowColumns(fixable: boolean): Column<ImageRow>[] {
  return [
    col.accessor("image", {
      header: "Image",
      enableHiding: false,
      cell: (c) => (
        <span className="flex flex-wrap items-center gap-2">
          <Link to={"/security/image" as never} search={{ image: c.getValue() } as never} className="font-ident hover:underline">
            {c.getValue()}
          </Link>
          {c.row.original.digests > 1 && <span className="rounded-full bg-step px-2 py-0.5 text-[11px] text-ink-muted">{c.row.original.digests} digests</span>}
        </span>
      ),
    }),
    col.accessor((r) => (fixable ? r.critical_fixable : r.critical), {
      id: "critical",
      header: "Critical",
      cell: (c) => <Count row={c.row.original} n={c.getValue()} tone="font-medium text-fall" />,
    }),
    col.accessor((r) => (fixable ? r.high_fixable : r.high), {
      id: "high",
      header: "High",
      cell: (c) => <Count row={c.row.original} n={c.getValue()} tone="font-medium text-state-wait" />,
    }),
    col.accessor("running", {
      header: "Running",
      cell: (c) =>
        c.getValue() > 0 ? (
          <span className="text-live">● {c.getValue()}</span>
        ) : (
          <span className="text-meta">not running</span>
        ),
    }),
    col.accessor((r) => r.envs.join(", "), { id: "where", header: "Where", cell: (c) => <span className="text-ink-muted">{c.getValue()}</span> }),
    col.accessor((r) => r.state, {
      id: "state",
      header: "State",
      cell: (c) => {
        const st = c.row.original.state;
        return (
          <span title={c.row.original.error ?? undefined}>
            <StateBadge state={STATE_LABEL[st]} tone={STATE_TONE[st]} />
          </span>
        );
      },
    }),
  ];
}

function Figure({ value, label, tone }: { value: number; label: string; tone?: string }) {
  return (
    <div className="grid rounded-lg bg-raised px-3 py-2">
      <span className={`font-figure text-lg font-semibold tabular-nums ${tone ?? ""}`}>{value}</span>
      <span className="text-xs text-meta">{label}</span>
    </div>
  );
}

function Totals({ s }: { s: SecuritySummary }) {
  return (
    <div role="group" aria-label="Totals" className="mb-4 flex flex-wrap gap-2">
      <Figure value={s.critical_fixable_running} label="critical, fixable, running" tone={s.critical_fixable_running ? "text-fall" : undefined} />
      <Figure value={s.scanned} label="images scanned" />
      <Figure value={s.failed} label="scan failed" tone={s.failed ? "text-fall" : undefined} />
      <Figure value={s.unscanned} label="not scanned" />
    </div>
  );
}

function Empty({ s }: { s: SecuritySummary | undefined }) {
  const me = useQuery(meQuery).data;
  const admin = me?.user.role === "admin";
  const settings = useQuery({ ...settingsQuery, enabled: admin }).data;
  const off = settings && !settings.envs.some((e) => e.effective_enabled);
  if (off) {
    return (
      <div className="rounded-lg bg-raised p-6 text-sm">
        <p className="font-medium">Security scanning is off.</p>
        <Link to={"/admin/security" as never} className="text-ink underline">
          Turn it on
        </Link>
      </div>
    );
  }
  if (s && s.scanned + s.unscanned + s.failed === 0) {
    return <p className="rounded-lg bg-raised p-6 text-sm text-ink-muted">Scanning… results appear here as each image finishes.</p>;
  }
  return null;
}

/** /security: images across the fleet, the ones to patch first on top. */
export function SecurityPage() {
  useSecurityLive();
  const search = securitySearch(useSearch({ strict: false }) as Record<string, unknown>);
  const navigate = useNavigate();
  const filter: ImagesFilter = {
    envs: search.envs ? search.envs.split(",") : [],
    running: search.running !== false,
    fixable: search.fixable !== false,
  };
  const set = (next: Partial<SecuritySearch>) =>
    void navigate({ to: "/security" as never, search: { ...search, ...next } as never, replace: true });
  const q = useQuery(imagesQuery(filter));
  const envs = useQuery(envsQuery).data ?? [];
  const canScan = useCanScan();
  const one = filter.envs.length === 1 ? filter.envs[0] : undefined;
  const scan = useMutation({
    mutationFn: (items: { env: string; images?: string[] }[]) => Promise.all(items.map((i) => scanNow(i))),
    onSuccess: (r) => toast.success(`Queued ${r.reduce((n, x) => n + x.queued, 0)} scans`),
    onError: toastError,
  });
  const scanRows = (rows: ImageRow[]) => {
    const byEnv = new Map<string, string[]>();
    for (const r of rows) for (const e of r.envs) if (canScan(e)) byEnv.set(e, [...(byEnv.get(e) ?? []), r.image]);
    scan.mutate([...byEnv].map(([env, images]) => ({ env, images })));
  };
  const cols = rowColumns(filter.fixable);
  return (
    <Page
      title="Security"
      kicker="Fleet"
      actions={
        one &&
        canScan(one) && (
          <Button variant="outline" disabled={scan.isPending} onClick={() => scan.mutate([{ env: one }])}>
            <ScanSearch aria-hidden />
            Scan environment
          </Button>
        )
      }
    >
      {q.data && <Totals s={q.data} />}
      <div className="mb-3 flex flex-wrap items-center gap-4 text-sm">
        <NativeSelect aria-label="Environment" value={one ?? ""} onChange={(e) => set({ envs: e.target.value || undefined })}>
          <option value="">All environments</option>
          {envs.map((e) => (
            <option key={e.id} value={e.name}>
              {e.name}
            </option>
          ))}
        </NativeSelect>
        <label className="flex items-center gap-2">
          <Checkbox checked={filter.running} onCheckedChange={(v) => set({ running: v === true ? undefined : false })} />
          Running only
        </label>
        <label className="flex items-center gap-2">
          <Checkbox checked={filter.fixable} onCheckedChange={(v) => set({ fixable: v === true ? undefined : false })} />
          Fix available
        </label>
      </div>
      {q.data && q.data.rows.length === 0 && <Empty s={q.data} />}
      {(!q.data || q.data.rows.length > 0) && (
        <DataTable
          label="Patch queue"
          data={q.data?.rows}
          error={q.error}
          columns={cols}
          getRowId={(r) => r.image}
          empty="Nothing to patch with these filters."
          selectable
          actions={(selected, clear) => (
            <Button
              size="sm"
              disabled={scan.isPending || !selected.some((r) => r.envs.some(canScan))}
              onClick={() => {
                scanRows(selected);
                clear();
              }}
            >
              <ScanSearch aria-hidden />
              Scan selected
            </Button>
          )}
        />
      )}
    </Page>
  );
}
