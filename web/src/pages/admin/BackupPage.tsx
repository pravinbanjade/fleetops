import { Download, Upload } from "lucide-react";
import { useState } from "react";
import { Page } from "@/components/Page";
import { Panel } from "@/components/Panel";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Slot } from "@/ee-slots";
import { downloadBackup, MIN_PASSPHRASE } from "./backup-api";
import { BackupUpload } from "./BackupUpload";
import { FormDialog } from "./FormDialog";

export function DownloadDialog({ onClose }: { onClose: () => void }) {
  const [passphrase, setPassphrase] = useState("");
  const [again, setAgain] = useState("");
  return (
    <FormDialog
      title="Download backup"
      description="FleetOps can't recover a lost passphrase. Keep it somewhere other than this server."
      submitLabel="Download"
      onClose={onClose}
      onSubmit={async () => {
        if (passphrase.length < MIN_PASSPHRASE) throw new Error(`The passphrase needs at least ${MIN_PASSPHRASE} characters.`);
        if (passphrase !== again) throw new Error("The passphrases do not match.");
        await downloadBackup(passphrase);
      }}
    >
      <div className="grid gap-2">
        <Label htmlFor="dl-passphrase">Passphrase</Label>
        <Input id="dl-passphrase" type="password" autoComplete="new-password" value={passphrase} onChange={(e) => setPassphrase(e.target.value)} />
      </div>
      <div className="grid gap-2">
        <Label htmlFor="dl-again">Passphrase again</Label>
        <Input id="dl-again" type="password" autoComplete="new-password" value={again} onChange={(e) => setAgain(e.target.value)} />
      </div>
    </FormDialog>
  );
}

/** Admin → Backup: download a backup, restore one; Enterprise adds scheduling. */
export function BackupPage() {
  const [downloading, setDownloading] = useState(false);
  return (
    <Page title="Backup">
      <div className="grid items-start gap-5 lg:grid-cols-2">
        <Panel title="Download backup" icon={Download}>
          <div className="grid gap-3">
            <p className="text-sm text-muted-foreground">
              One file with every user, environment, grant, setting and the audit log, locked with a passphrase. Restore
              it here or on a new server's first screen.
            </p>
            <Button className="justify-self-start" onClick={() => setDownloading(true)}>
              Download backup
            </Button>
          </div>
          {downloading && <DownloadDialog onClose={() => setDownloading(false)} />}
        </Panel>
        <Panel title="Restore" icon={Upload}>
          <BackupUpload mode="admin" />
        </Panel>
      </div>
      <Slot name="admin.backup" />
    </Page>
  );
}
