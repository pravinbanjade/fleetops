import { useMutation } from "@tanstack/react-query";
import { useState } from "react";
import type { StagedBackup } from "@/api/gen/types.gen";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { isEnterpriseBuild } from "@/ee-slots";
import { errorMessage } from "@/lib/errors";
import { formatTime } from "@/lib/format";
import { FormError } from "@/pages/AuthLayout";
import { discardStaged, restoreStaged, waitForRestart } from "./backup-api";

const SHOWN: [string, string][] = [
  ["users", "Users"],
  ["environments", "Environments"],
  ["audit_log", "Audit entries"],
];

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
  const s = staged.summary;
  const m = useMutation({
    mutationFn: async () => {
      await restoreStaged(staged.id, mode === "setup");
      await waitForRestart();
    },
    meta: { inline: true },
    onSuccess: () => {
      window.location.assign("/login");
    },
  });
  const cancel = () => {
    if (mode === "admin") void discardStaged(staged.id).catch(() => undefined);
    onCancel();
  };
  if (m.isPending) {
    return (
      <div role="status" className="grid gap-2 text-sm">
        <p className="font-medium">Restoring…</p>
        <p className="text-muted-foreground">FleetOps is restarting with the backup. You'll be sent to sign in.</p>
      </div>
    );
  }
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
            <dd>{s.counts[k] ?? 0}</dd>
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
          Restore
        </Button>
        <Button variant="outline" onClick={cancel}>
          Cancel
        </Button>
      </div>
    </div>
  );
}
