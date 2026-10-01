import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState, type FormEvent } from "react";
import { toast } from "sonner";
import { keys, sessionsQuery } from "@/api/queries";
import { changePassword, revokeSession } from "@/api/gen/sdk.gen";
import { useLogout, useSession } from "@/auth/session";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { errorMessage } from "@/lib/errors";
import { formatTime } from "@/lib/format";
import { FormError } from "./AuthLayout";
import { UserAvatar } from "@/components/UserAvatar";

export function AccountPage() {
  const { user } = useSession();
  return (
    <div className="grid max-w-3xl gap-4 p-6">
      <h1 className="text-2xl font-semibold">Account</h1>
      <p className="text-sm text-muted-foreground">
        <span className="inline-flex items-center gap-2">
          <UserAvatar name={user.username} size="sm" />
          <span>
            Signed in as <span className="font-medium text-foreground">{user.username}</span> ({user.role})
          </span>
        </span>
      </p>
      <PasswordCard />
      <SessionsCard />
    </div>
  );
}

function PasswordCard() {
  const [current, setCurrent] = useState("");
  const [next, setNext] = useState("");
  const [confirm, setConfirm] = useState("");
  const [mismatch, setMismatch] = useState(false);
  const m = useMutation({
    mutationFn: () => changePassword({ body: { current_password: current, new_password: next } }),
    onSuccess: () => {
      setCurrent("");
      setNext("");
      setConfirm("");
      toast.success("Password changed");
    },
  });
  const submit = (e: FormEvent) => {
    e.preventDefault();
    setMismatch(next !== confirm);
    if (next === confirm) m.mutate();
  };
  return (
    <Card>
      <CardHeader>
        <CardTitle>Change password</CardTitle>
      </CardHeader>
      <CardContent>
        <form onSubmit={submit} className="grid max-w-sm gap-4">
          <div className="grid gap-2">
            <Label htmlFor="current-password">Current password</Label>
            <Input
              id="current-password"
              type="password"
              autoComplete="current-password"
              required
              value={current}
              onChange={(e) => setCurrent(e.target.value)}
            />
          </div>
          <div className="grid gap-2">
            <Label htmlFor="new-password">New password</Label>
            <Input
              id="new-password"
              type="password"
              autoComplete="new-password"
              required
              value={next}
              onChange={(e) => setNext(e.target.value)}
            />
          </div>
          <div className="grid gap-2">
            <Label htmlFor="confirm-password">Confirm new password</Label>
            <Input
              id="confirm-password"
              type="password"
              autoComplete="new-password"
              required
              value={confirm}
              onChange={(e) => setConfirm(e.target.value)}
            />
          </div>
          <FormError message={mismatch ? "The passwords do not match." : m.error ? errorMessage(m.error) : null} />
          <Button type="submit" className="justify-self-start" disabled={m.isPending}>
            Change password
          </Button>
        </form>
      </CardContent>
    </Card>
  );
}

function SessionsCard() {
  const qc = useQueryClient();
  const sessions = useQuery(sessionsQuery);
  const logout = useLogout();
  const revoke = useMutation({
    mutationFn: (id: number) => revokeSession({ path: { id } }),
    onSuccess: () => void qc.invalidateQueries({ queryKey: keys.sessions }),
  });
  return (
    <Card>
      <CardHeader>
        <CardTitle>Sessions</CardTitle>
      </CardHeader>
      <CardContent>
        {sessions.error && <FormError message={errorMessage(sessions.error)} />}
        <ul className="grid gap-3" aria-label="Sessions">
          {sessions.data?.map((s) => (
            <li key={s.id} className="flex items-center justify-between gap-4 text-sm">
              <div className="min-w-0">
                <div className="truncate font-medium">
                  {s.user_agent ?? "Unknown browser"}
                  {s.current && <span className="ml-2 text-xs text-muted-foreground">(this session)</span>}
                </div>
                <div className="text-muted-foreground">
                  {s.ip ?? "unknown address"} · last seen {formatTime(s.last_seen_at)}
                </div>
              </div>
              {s.current ? (
                <Button variant="outline" size="sm" onClick={() => logout.mutate()}>
                  Log out
                </Button>
              ) : (
                <Button variant="outline" size="sm" disabled={revoke.isPending} onClick={() => revoke.mutate(s.id)}>
                  Revoke
                </Button>
              )}
            </li>
          ))}
        </ul>
      </CardContent>
    </Card>
  );
}
