import { useState } from "react";
import { Page } from "@/components/Page";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Slot } from "@/ee-slots";
import { downloadBackup, MIN_PASSPHRASE } from "./backup-api";
import { BackupUpload } from "./BackupUpload";
import { FormDialog } from "./FormDialog";

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section className="grid max-w-2xl gap-3 rounded-xl border p-5">
      <h2 className="font-semibold">{title}</h2>
      {children}
    </section>
  );
}

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
      <Section title="Download backup">
        <p className="text-sm text-muted-foreground">
          One file with every user, environment, grant, setting and the audit log, locked with a passphrase. Restore it
          here or on a new server's first screen.
        </p>
        <Button className="justify-self-start" onClick={() => setDownloading(true)}>
          Download backup
        </Button>
        {downloading && <DownloadDialog onClose={() => setDownloading(false)} />}
      </Section>
      <Section title="Restore">
        <BackupUpload mode="admin" />
      </Section>
      <Slot name="admin.backup" />
    </Page>
  );
}
