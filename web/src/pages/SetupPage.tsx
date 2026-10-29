import { Plus } from "lucide-react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { getRouteApi, useNavigate } from "@tanstack/react-router";
import { useState, type FormEvent } from "react";
import { setCsrfToken } from "@/api/client";
import { keys } from "@/api/queries";
import { setup } from "@/api/gen/sdk.gen";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { errorMessage } from "@/lib/errors";
import { AuthLayout, FormError } from "./AuthLayout";
import { BackupUpload } from "./admin/BackupUpload";

const route = getRouteApi("/setup");

export function SetupPage() {
  const { setupOpen } = route.useRouteContext();
  const navigate = useNavigate();
  const qc = useQueryClient();
  const [username, setUsername] = useState("admin");
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [mismatch, setMismatch] = useState(false);
  const [restoring, setRestoring] = useState(false);
  const m = useMutation({
    mutationFn: async () => (await setup({ body: { username, password } })).data,
    onSuccess: (session) => {
      setCsrfToken(session.csrf_token);
      qc.setQueryData(keys.me, session);
      qc.setQueryData(keys.authStatus, { setup_required: false, setup_open: false });
      void navigate({ to: "/" });
    },
  });

  if (!setupOpen) {
    return (
      <AuthLayout title="Setup is locked">
        <p className="text-sm text-muted-foreground">
          No administrator was created within 10 minutes of FleetOps starting, so setup was locked to stop anyone else
          claiming this instance. Restart FleetOps and create the administrator straight away.
        </p>
      </AuthLayout>
    );
  }

  const submit = (e: FormEvent) => {
    e.preventDefault();
    setMismatch(password !== confirm);
    if (password === confirm) m.mutate();
  };

  const choice = (
    <div role="group" aria-label="Get started" className="grid grid-cols-2 gap-1 rounded-lg bg-raised p-1 text-sm">
      {[
        [false, "Start fresh"],
        [true, "Restore from backup"],
      ].map(([value, label]) => (
        <button
          key={String(label)}
          type="button"
          aria-pressed={restoring === value}
          className="rounded-md px-2 py-1.5 aria-pressed:bg-muted aria-pressed:font-medium"
          onClick={() => setRestoring(value as boolean)}
        >
          {label}
        </button>
      ))}
    </div>
  );

  if (restoring) {
    return (
      <AuthLayout title="Welcome to FleetOps" description="Restore this server from a backup file.">
        {choice}
        <BackupUpload mode="setup" />
      </AuthLayout>
    );
  }

  return (
    <AuthLayout title="Welcome to FleetOps" description="Create the administrator account.">
      {choice}
      <form onSubmit={submit} className="grid gap-4">
        <div className="grid gap-2">
          <Label htmlFor="username">Username</Label>
          <Input id="username" autoComplete="username" required value={username} onChange={(e) => setUsername(e.target.value)} />
        </div>
        <div className="grid gap-2">
          <Label htmlFor="password">Password</Label>
          <Input
            id="password"
            type="password"
            autoComplete="new-password"
            required
            value={password}
            onChange={(e) => setPassword(e.target.value)}
          />
        </div>
        <div className="grid gap-2">
          <Label htmlFor="confirm">Confirm password</Label>
          <Input
            id="confirm"
            type="password"
            autoComplete="new-password"
            required
            value={confirm}
            onChange={(e) => setConfirm(e.target.value)}
          />
        </div>
        <FormError message={mismatch ? "The passwords do not match." : m.error ? errorMessage(m.error) : null} />
        <Button type="submit" disabled={m.isPending}>
          <Plus aria-hidden />
          {m.isPending ? "Creating…" : "Create administrator"}
        </Button>
      </form>
    </AuthLayout>
  );
}
