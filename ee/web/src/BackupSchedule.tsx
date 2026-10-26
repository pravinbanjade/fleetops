import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState, type ReactNode } from "react";
import type { StagedBackup } from "@/api/gen/types.gen";
import { ArchiveRestore, CalendarClock, ChevronsDown, FlaskConical, HardDrive, History, KeyRound, ListChecks, Play, PowerOff, Save, Trash2, type LucideIcon } from "lucide-react";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { Panel } from "@/components/Panel";
import { StateBadge } from "@/components/StateBadge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { NativeSelect } from "@/components/ui/native-select";
import { errorMessage, toastError } from "@/lib/errors";
import { formatBytes, formatTime } from "@/lib/format";
import { FormError } from "@/pages/AuthLayout";
import { MIN_PASSPHRASE } from "@/pages/admin/backup-api";
import { FormDialog } from "@/pages/admin/FormDialog";
import { RestoreFlow } from "@/pages/admin/RestoreFlow";
import {
  backupKeys,
  deleteTarget,
  localTimeOf,
  putPassphrase,
  putSchedule,
  putTarget,
  recentQuery,
  runNow,
  runsQuery,
  scheduleQuery,
  stageRecent,
  statusQuery,
  targetsQuery,
  testTarget,
  viewerOffset,
  type BackupTarget,
  type RecentBackup,
  type Retention,
  type TargetKind,
  type TargetSettings,
} from "./backup";
import { FrozenNotice } from "./FrozenNotice";

const KIND_LABEL: Record<TargetKind, string> = { s3: "S3", local: "Local directory" };

/** Remove, after saying what stops; the last target also turns the schedule off. */
function RemoveTarget({ kind }: { kind: TargetKind }) {
  const [open, setOpen] = useState(false);
  const invalidate = useInvalidate();
  const targets = useQuery(targetsQuery).data ?? [];
  const scheduled = useQuery(scheduleQuery).data?.enabled ?? false;
  const last = targets.length === 1 && targets[0].kind === kind;
  return (
    <>
      <Button size="sm" variant="outline" onClick={() => setOpen(true)}>
        <Trash2 aria-hidden />
        Remove
      </Button>
      <ConfirmDialog
        open={open}
        onOpenChange={setOpen}
        title={`Remove the ${KIND_LABEL[kind]} target?`}
        description={[
          `FleetOps forgets these settings${kind === "s3" ? " and the saved secret" : ""} and stops writing backups there.`,
          "Backups already written stay where they are.",
          last && scheduled ? "This is the last target, so scheduled backups turn off." : "",
        ]
          .filter(Boolean)
          .join(" ")}
        confirmLabel="Remove target"
        onConfirm={() => deleteTarget(kind).then(invalidate)}
      />
    </>
  );
}

function Section({ title, icon, actions, children }: { title: string; icon?: LucideIcon; actions?: ReactNode; children: ReactNode }) {
  return (
    <Panel title={title} icon={icon} actions={actions}>
      <div className="grid gap-3">{children}</div>
    </Panel>
  );
}

function useInvalidate() {
  const qc = useQueryClient();
  return () => qc.invalidateQueries({ queryKey: backupKeys.all });
}

/** Admin → Backup, Enterprise: the schedule, its targets and the recent backups. */
export function BackupSchedule() {
  const status = useQuery(statusQuery);
  if (!status.data) return null;
  return status.data.licensed ? <Licensed /> : <Unlicensed />;
}

/** Without a license: read-only, and only turning things off. */
function Unlicensed() {
  const schedule = useQuery(scheduleQuery).data;
  const targets = useQuery(targetsQuery).data ?? [];
  const invalidate = useInvalidate();
  const off = useMutation({
    mutationFn: () => putSchedule({ enabled: false, hour_utc: schedule!.hour_utc, retention: schedule!.retention }),
    onSuccess: invalidate,
    onError: toastError,
  });
  return (
    <Section title="Scheduled backups" icon={CalendarClock}>
      <FrozenNotice>Scheduled backups need a license. You can turn them off and remove targets.</FrozenNotice>
      {schedule?.enabled && (
        <Button variant="outline" className="justify-self-start" onClick={() => off.mutate()}>
          <PowerOff aria-hidden />
          Turn off scheduled backups
        </Button>
      )}
      {targets.map((t) => (
        <div key={t.kind} className="flex items-center justify-between gap-2 text-sm">
          <span>{KIND_LABEL[t.kind]}</span>
          <RemoveTarget kind={t.kind} />
        </div>
      ))}
    </Section>
  );
}

/** When and where on the left, what happened on the right. */
function Licensed() {
  return (
    <div className="grid items-start gap-5 lg:grid-cols-2">
      <div className="grid gap-5">
        <ScheduleForm />
        <Targets />
      </div>
      <div className="grid gap-5">
        <Runs />
        <Recent />
      </div>
    </div>
  );
}

function ScheduleForm() {
  const schedule = useQuery(scheduleQuery).data;
  const invalidate = useInvalidate();
  const [enabled, setEnabled] = useState(false);
  const [hour, setHour] = useState(2);
  const [retention, setRetention] = useState<Retention>({ kind: "count", value: 7 });
  const [passphraseOpen, setPassphraseOpen] = useState(false);
  useEffect(() => {
    if (!schedule) return;
    setEnabled(schedule.enabled);
    setHour(schedule.hour_utc);
    setRetention(schedule.retention);
  }, [schedule]);
  const save = useMutation({
    mutationFn: () => putSchedule({ enabled, hour_utc: hour, retention }),
    meta: { inline: true },
    onSuccess: invalidate,
  });
  if (!schedule) return null;
  const offset = viewerOffset();
  const dirty =
    enabled !== schedule.enabled ||
    hour !== schedule.hour_utc ||
    retention.kind !== schedule.retention.kind ||
    retention.value !== schedule.retention.value;
  return (
    <Section title="Scheduled backups" icon={CalendarClock}>
      <div className="flex items-center gap-2">
        <Checkbox id="bk-enabled" checked={enabled} onCheckedChange={(v) => setEnabled(!!v)} />
        <Label htmlFor="bk-enabled">Back up every day</Label>
      </div>
      <div className="grid gap-2 sm:grid-cols-2">
        <div className="grid gap-2">
          <Label htmlFor="bk-hour">At (your time)</Label>
          <NativeSelect id="bk-hour" value={hour} onChange={(e) => setHour(Number(e.target.value))}>
            {Array.from({ length: 24 }, (_, h) => (
              <option key={h} value={h}>
                {localTimeOf(h, offset)} ({String(h).padStart(2, "0")}:00 UTC)
              </option>
            ))}
          </NativeSelect>
        </div>
        <div className="grid gap-2">
          <Label htmlFor="bk-keep">Keep</Label>
          <div className="flex gap-2">
            <Input
              id="bk-keep"
              type="number"
              min={1}
              className="w-24"
              value={retention.value}
              onChange={(e) => setRetention({ ...retention, value: Number(e.target.value) })}
            />
            <NativeSelect
              aria-label="Keep by"
              value={retention.kind}
              onChange={(e) => setRetention({ kind: e.target.value as Retention["kind"], value: retention.value })}
            >
              <option value="count">latest backups</option>
              <option value="days">days of backups</option>
            </NativeSelect>
          </div>
        </div>
      </div>
      <div className="flex flex-wrap items-center gap-2 text-sm">
        <span>Passphrase:</span>
        <StateBadge state={schedule.passphrase_set ? "set" : "not set"} tone={schedule.passphrase_set ? "run" : "fail"} />
        <Button variant="outline" size="sm" onClick={() => setPassphraseOpen(true)}>
          <KeyRound aria-hidden />
          {schedule.passphrase_set ? "Replace passphrase" : "Set passphrase"}
        </Button>
      </div>
      <FormError message={save.error ? errorMessage(save.error) : null} />
      <div className="flex flex-wrap items-center gap-3">
        <Button disabled={save.isPending || !dirty} onClick={() => save.mutate()}>
          <Save aria-hidden />
          Save schedule
        </Button>
        {dirty && <span className="text-sm text-state-wait-text">Unsaved changes</span>}
      </div>
      {passphraseOpen && <PassphraseDialog onClose={() => setPassphraseOpen(false)} />}
    </Section>
  );
}

function PassphraseDialog({ onClose }: { onClose: () => void }) {
  const invalidate = useInvalidate();
  const [p, setP] = useState("");
  const [again, setAgain] = useState("");
  return (
    <FormDialog
      title="Backup passphrase"
      description="Scheduled backups are locked with it, and you need it to restore them on another server. FleetOps can't recover it: keep it somewhere other than this server."
      submitLabel="Save passphrase"
      onClose={onClose}
      onSubmit={async () => {
        if (p.length < MIN_PASSPHRASE) throw new Error(`The passphrase needs at least ${MIN_PASSPHRASE} characters.`);
        if (p !== again) throw new Error("The passphrases do not match.");
        await putPassphrase(p);
        await invalidate();
      }}
    >
      <div className="grid gap-2">
        <Label htmlFor="bk-pass">Passphrase</Label>
        <Input id="bk-pass" type="password" autoComplete="new-password" value={p} onChange={(e) => setP(e.target.value)} />
      </div>
      <div className="grid gap-2">
        <Label htmlFor="bk-pass-again">Passphrase again</Label>
        <Input id="bk-pass-again" type="password" autoComplete="new-password" value={again} onChange={(e) => setAgain(e.target.value)} />
      </div>
    </FormDialog>
  );
}

function Targets() {
  const targets = useQuery(targetsQuery).data;
  if (!targets) return null;
  const of = (k: TargetKind) => targets.find((t) => t.kind === k);
  return (
    <Section title="Where backups go" icon={HardDrive}>
      <S3Form target={of("s3")} />
      <LocalForm target={of("local")} />
    </Section>
  );
}

function TargetActions({
  kind,
  saved,
  dirty,
  onSave,
  saving,
}: {
  kind: TargetKind;
  saved: boolean;
  dirty: boolean;
  onSave: () => void;
  saving: boolean;
}) {
  const test = useMutation({ mutationFn: () => testTarget(kind), meta: { inline: true } });
  return (
    <div className="grid gap-2">
      <div className="flex flex-wrap gap-2">
        <Button size="sm" disabled={saving || !dirty} onClick={onSave}>
          <Save aria-hidden />
          Save
        </Button>
        {saved && (
          <>
            <Button size="sm" variant="outline" disabled={test.isPending} onClick={() => test.mutate()}>
              <FlaskConical aria-hidden />
              Test
            </Button>
            <RemoveTarget kind={kind} />
          </>
        )}
        {dirty && <span className="self-center text-sm text-state-wait-text">Unsaved changes</span>}
      </div>
      {test.isSuccess && <p className="text-sm text-muted-foreground">It works: FleetOps wrote and deleted a test file.</p>}
      <FormError message={test.error ? errorMessage(test.error) : null} />
    </div>
  );
}

function Field(props: { id: string; label: string; value: string; onChange: (v: string) => void; placeholder?: string; type?: string }) {
  return (
    <div className="grid gap-1.5">
      <Label htmlFor={props.id}>{props.label}</Label>
      <Input
        id={props.id}
        type={props.type}
        placeholder={props.placeholder}
        autoComplete="off"
        value={props.value}
        onChange={(e) => props.onChange(e.target.value)}
      />
    </div>
  );
}

function S3Form({ target }: { target?: BackupTarget }) {
  const invalidate = useInvalidate();
  const init = target?.settings.kind === "s3" ? target.settings : undefined;
  const [endpoint, setEndpoint] = useState(init?.endpoint ?? "");
  const [region, setRegion] = useState(init?.region ?? "");
  const [bucket, setBucket] = useState(init?.bucket ?? "");
  const [prefix, setPrefix] = useState(init?.prefix ?? "");
  const [pathStyle, setPathStyle] = useState(init?.path_style ?? false);
  const [keyId, setKeyId] = useState(init?.access_key_id ?? "");
  const [secret, setSecret] = useState("");
  const save = useMutation({
    mutationFn: () =>
      putTarget(
        { kind: "s3", endpoint: endpoint || null, region, bucket, prefix, path_style: pathStyle, access_key_id: keyId },
        secret || undefined,
      ),
    meta: { inline: true },
    onSuccess: () => {
      setSecret("");
      void invalidate();
    },
  });
  return (
    <fieldset className="grid gap-3 rounded-lg bg-[#1c1c1c] p-4">
      <legend className="px-1 text-sm font-medium">S3-compatible storage</legend>
      <div className="grid gap-3 sm:grid-cols-2">
        <Field id="s3-endpoint" label="Endpoint" placeholder="Blank for AWS" value={endpoint} onChange={setEndpoint} />
        <Field id="s3-region" label="Region" placeholder="us-east-1" value={region} onChange={setRegion} />
        <Field id="s3-bucket" label="Bucket" value={bucket} onChange={setBucket} />
        <Field id="s3-prefix" label="Prefix" placeholder="Optional" value={prefix} onChange={setPrefix} />
        <Field id="s3-key" label="Access key ID" value={keyId} onChange={setKeyId} />
        <Field
          id="s3-secret"
          label="Secret access key"
          type="password"
          placeholder={target?.secret_set ? "Saved; leave blank to keep" : ""}
          value={secret}
          onChange={setSecret}
        />
      </div>
      <div className="flex items-center gap-2">
        <Checkbox id="s3-path" checked={pathStyle} onCheckedChange={(v) => setPathStyle(!!v)} />
        <Label htmlFor="s3-path">Path-style URLs (MinIO and similar)</Label>
      </div>
      <FormError message={save.error ? errorMessage(save.error) : null} />
      <TargetActions
        kind="s3"
        saved={!!target}
        dirty={
          !!secret ||
          endpoint !== (init?.endpoint ?? "") ||
          region !== (init?.region ?? "") ||
          bucket !== (init?.bucket ?? "") ||
          prefix !== (init?.prefix ?? "") ||
          pathStyle !== (init?.path_style ?? false) ||
          keyId !== (init?.access_key_id ?? "")
        }
        saving={save.isPending}
        onSave={() => save.mutate()}
      />
    </fieldset>
  );
}

function LocalForm({ target }: { target?: BackupTarget }) {
  const invalidate = useInvalidate();
  const [path, setPath] = useState(target?.settings.kind === "local" ? target.settings.path : "");
  const save = useMutation({
    mutationFn: () => putTarget({ kind: "local", path } satisfies TargetSettings),
    meta: { inline: true },
    onSuccess: invalidate,
  });
  return (
    <fieldset className="grid gap-3 rounded-lg bg-[#1c1c1c] p-4">
      <legend className="px-1 text-sm font-medium">Local directory</legend>
      <Field id="local-path" label="Directory" placeholder="/mnt/backups" value={path} onChange={setPath} />
      <p className="text-xs text-muted-foreground">A mounted volume or network share, outside FleetOps' data directory.</p>
      <FormError message={save.error ? errorMessage(save.error) : null} />
      <TargetActions
        kind="local"
        saved={!!target}
        dirty={path !== (target?.settings.kind === "local" ? target.settings.path : "")}
        saving={save.isPending}
        onSave={() => save.mutate()}
      />
    </fieldset>
  );
}

function Runs() {
  const status = useQuery({ ...statusQuery, refetchInterval: 5000 }).data;
  const runs = useQuery({ ...runsQuery, refetchInterval: 5000 }).data ?? [];
  const invalidate = useInvalidate();
  const run = useMutation({ mutationFn: runNow, meta: { inline: true }, onSuccess: invalidate });
  const [all, setAll] = useState(false);
  const shown = all ? runs : runs.slice(0, 6);
  return (
    <Section title="Runs" icon={ListChecks}>
      <div className="flex flex-wrap items-center gap-3 text-sm">
        <Button size="sm" variant="outline" disabled={run.isPending} onClick={() => run.mutate()}>
          <Play aria-hidden />
          Run now
        </Button>
        {status?.next_run_at && <span className="text-muted-foreground">Next: {formatTime(status.next_run_at)}</span>}
      </div>
      <FormError message={run.error ? errorMessage(run.error) : null} />
      {runs.length === 0 ? (
        <p className="text-sm text-muted-foreground">No backups have run yet.</p>
      ) : (
        <ul className="grid gap-1 text-sm">
          {shown.map((r) => (
            <li key={r.id} className="flex flex-wrap items-center gap-2">
              <StateBadge state={r.result} tone={r.result === "failure" ? "fail" : r.result === "skipped" ? "wait" : "run"} />
              <span className="tabular-nums">{formatTime(r.started_at)}</span>
              {r.target_kind && <span className="text-muted-foreground">{KIND_LABEL[r.target_kind]}</span>}
              {r.error && <span className="text-state-fail-text">{r.error}</span>}
            </li>
          ))}
        </ul>
      )}
      {runs.length > 6 && (
        <Button variant="ghost" size="sm" className="justify-self-start" onClick={() => setAll(!all)}>
          <ChevronsDown aria-hidden />
          {all ? "Show the latest 6" : `Show all ${runs.length} runs`}
        </Button>
      )}
    </Section>
  );
}

/** The newest backups across targets, each restorable from here. */
function Recent() {
  const recent = useQuery(recentQuery).data;
  const [staged, setStaged] = useState<StagedBackup | null>(null);
  if (staged) {
    return (
      <Section title="Restore" icon={History}>
        <RestoreFlow staged={staged} mode="admin" onCancel={() => setStaged(null)} />
      </Section>
    );
  }
  return (
    <Section title="Recent backups" icon={History}>
      {recent?.errors.map((e) => (
        <p key={e.target_kind} className="text-sm text-state-fail-text">
          {KIND_LABEL[e.target_kind]}: {e.error}
        </p>
      ))}
      {recent && recent.backups.length === 0 && <p className="text-sm text-muted-foreground">No backups yet.</p>}
      <ul className="grid gap-2">
        {recent?.backups.map((b) => <RecentRow key={`${b.target_kind}/${b.name}`} backup={b} onStaged={setStaged} />)}
      </ul>
    </Section>
  );
}

function RecentRow({ backup, onStaged }: { backup: RecentBackup; onStaged: (s: StagedBackup) => void }) {
  const [asking, setAsking] = useState(false);
  const [passphrase, setPassphrase] = useState("");
  const m = useMutation({
    mutationFn: () => stageRecent(backup.target_kind, backup.name, asking ? passphrase : undefined),
    meta: { inline: true },
    onSuccess: onStaged,
    onError: (e) => {
      if (errorMessage(e).includes("different passphrase")) setAsking(true);
    },
  });
  return (
    <li className="grid gap-2 text-sm">
      <div className="flex flex-wrap items-center gap-2">
        <span>{formatTime(backup.created_at)}</span>
        <span className="text-muted-foreground">{KIND_LABEL[backup.target_kind]}</span>
        <span className="text-muted-foreground">{formatBytes(backup.size)}</span>
        <Button size="sm" variant="outline" disabled={m.isPending} onClick={() => m.mutate()}>
          <ArchiveRestore aria-hidden />
          Restore
        </Button>
      </div>
      {asking && (
        <div className="grid gap-1.5">
          <Label htmlFor={`pp-${backup.name}`}>This backup's passphrase</Label>
          <Input
            id={`pp-${backup.name}`}
            type="password"
            autoComplete="off"
            value={passphrase}
            onChange={(e) => setPassphrase(e.target.value)}
          />
        </div>
      )}
      <FormError message={m.error ? errorMessage(m.error) : null} />
    </li>
  );
}
