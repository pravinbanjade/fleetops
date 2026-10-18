import { useMutation } from "@tanstack/react-query";
import { useState, type FormEvent } from "react";
import type { StagedBackup } from "@/api/gen/types.gen";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { errorMessage } from "@/lib/errors";
import { FormError } from "@/pages/AuthLayout";
import { stageBackup } from "./backup-api";
import { RestoreFlow } from "./RestoreFlow";

/** Pick a backup file and its passphrase, see what it holds, restore it. */
export function BackupUpload({ mode }: { mode: "admin" | "setup" }) {
  const [file, setFile] = useState<File | null>(null);
  const [passphrase, setPassphrase] = useState("");
  const [staged, setStaged] = useState<StagedBackup | null>(null);
  const m = useMutation({
    mutationFn: () => stageBackup(file!, passphrase, mode === "setup"),
    meta: { inline: true },
    onSuccess: setStaged,
  });
  if (staged) return <RestoreFlow staged={staged} mode={mode} onCancel={() => setStaged(null)} />;
  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (file) m.mutate();
  };
  return (
    <form onSubmit={submit} className="grid gap-4">
      <div className="grid gap-2">
        <Label htmlFor="backup-file">Backup file</Label>
        <Input id="backup-file" type="file" accept=".fob" required onChange={(e) => setFile(e.target.files?.[0] ?? null)} />
      </div>
      <div className="grid gap-2">
        <Label htmlFor="backup-passphrase">Passphrase</Label>
        <Input
          id="backup-passphrase"
          type="password"
          autoComplete="off"
          required
          value={passphrase}
          onChange={(e) => setPassphrase(e.target.value)}
        />
      </div>
      <FormError message={m.error ? errorMessage(m.error) : null} />
      <Button type="submit" variant="outline" className="justify-self-start" disabled={m.isPending || !file}>
        {m.isPending ? "Opening…" : "Open backup"}
      </Button>
    </form>
  );
}
