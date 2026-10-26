import { useMutation, useQuery } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { envsQuery, usersQuery } from "@/api/queries";
import type { StagedBackup } from "@/api/gen/types.gen";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { isEnterpriseBuild } from "@/ee-slots";
import { errorMessage } from "@/lib/errors";
import { formatTime } from "@/lib/format";
import { FormError } from "@/pages/AuthLayout";
import { discardStaged, noteRestored, restoreStaged, waitForRestart } from "./backup-api";

const SHOWN: [string, string][] = [
  ["users", "Users"],
  ["environments", "Environments"],
  ["audit_log", "Audit entries"],
];

/** After this long a restart is overdue and the admin is told where to look. */
const SLOW_SECS = 60;

/** Seconds since mount, ticking once a second. */
function useElapsed() {
  const [secs, setSecs] = useState(0);
  useEffect(() => {
    const start = Date.now();
    const t = setInterval(() => setSecs(Math.floor((Date.now() - start) / 1000)), 1000);
    return () => clearInterval(t);
  }, []);
  return secs;
}

/** The wait for the restarted server, with advice once it runs long. */
function Restarting({ phase }: { phase: "restoring" | "restarting" }) {
  const secs = useElapsed();
  return (
    <div role="status" className="grid gap-2 text-sm">
      <p className="font-medium">
        {phase === "restoring" ? "Restoring…" : "Waiting for FleetOps to restart…"}{" "}
        <span className="font-normal text-muted-foreground tabular-nums">{secs}s</span>
      </p>
      <p className="text-muted-foreground">FleetOps restarts with the backup. You'll be sent to sign in.</p>
      {secs >= SLOW_SECS && (
        <div className="grid gap-2">
          <p>
            Still waiting. The server may have failed to start: check its logs (for example{" "}
            <code className="font-mono">docker logs fleetops</code>). This page keeps checking.
          </p>
          <Button variant="outline" size="sm" className="justify-self-start" asChild>
            <a href="/login">Go to sign in</a>
          </Button>
        </div>
      )}
    </div>
  );
}

/** How many users and environments this server has now, for the admin's comparison. */
function useCurrentCounts(on: boolean): Record<string, number | undefined> {
  const users = useQuery({ ...usersQuery, enabled: on });
  const envs = useQuery({ ...envsQuery, enabled: on });
  return { users: users.data?.length, environments: envs.data?.length };
}

/**
 * What a staged backup holds, its warnings, and the restore itself. Over a
 * running install the admin types RESTORE first.
 */
export function RestoreFlow({
  staged,
  mode,
  onCancel,
}: {
  staged: StagedBackup;
  mode: "admin" | "setup";
  onCancel: () => void;
}) {
  const [confirm, setConfirm] = useState("");
  const [phase, setPhase] = useState<"restoring" | "restarting">("restoring");
  const s = staged.summary;
  const admin = mode === "admin";
  const now = useCurrentCounts(admin);
  const m = useMutation({
    mutationFn: async () => {
      setPhase("restoring");
      await restoreStaged(staged.id, mode === "setup");
      setPhase("restarting");
      await waitForRestart();
    },
    meta: { inline: true },
    onSuccess: () => {
      noteRestored(s.created_at);
      window.location.assign("/login");
    },
  });
  const cancel = () => {
    if (mode === "admin") void discardStaged(staged.id).catch(() => undefined);
    onCancel();
  };
  if (m.isPending) return <Restarting phase={phase} />;
  return (
    <div className="grid gap-4 text-sm">
      <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1">
        <dt className="text-muted-foreground">Made</dt>
        <dd>
          {formatTime(s.created_at)} by FleetOps {s.fleetops_version}
        </dd>
        <dt className="text-muted-foreground">Edition</dt>
        <dd>{s.edition === "enterprise" ? "Enterprise" : "Community"}</dd>
        {s.source && (
          <>
            <dt className="text-muted-foreground">From</dt>
            <dd>{s.source}</dd>
          </>
        )}
        {SHOWN.map(([k, label]) => (
          <div key={k} className="contents">
            <dt className="text-muted-foreground">{label}</dt>
            <dd className="tabular-nums">
              {s.counts[k] ?? 0}
              {admin && now[k] !== undefined && now[k] !== (s.counts[k] ?? 0) && (
                <span className="text-muted-foreground"> (this server has {now[k]})</span>
              )}
            </dd>
          </div>
        ))}
      </dl>
      <ul className="grid list-disc gap-1 pl-5 text-muted-foreground">
        {mode === "admin" && <li>Everything on this server is replaced, and everyone is signed out.</li>}
        <li>Agents enrolled after this backup was made can't connect and need enrolling again.</li>
        {s.has_enterprise_data && !isEnterpriseBuild && (
          <li>
            This backup has Enterprise data (teams, roles, single sign-on). Community keeps it but doesn't use it until
            you run the Enterprise edition.
          </li>
        )}
        {s.has_enterprise_data && (
          <li>Scheduled-backup settings come along: if the old server still runs, turn one of them off.</li>
        )}
      </ul>
      {mode === "admin" && (
        <div className="grid gap-2">
          <Label htmlFor="restore-confirm">Type RESTORE to confirm</Label>
          <Input id="restore-confirm" autoComplete="off" value={confirm} onChange={(e) => setConfirm(e.target.value)} />
        </div>
      )}
      <FormError message={m.error ? errorMessage(m.error) : null} />
      <div className="flex gap-2">
        <Button
          variant={mode === "admin" ? "destructive" : "default"}
          disabled={mode === "admin" && confirm !== "RESTORE"}
          onClick={() => m.mutate()}
        >
          {admin ? "Replace this server with the backup" : "Restore"}
        </Button>
        <Button variant="outline" onClick={cancel}>
          Cancel
        </Button>
      </div>
    </div>
  );
}
