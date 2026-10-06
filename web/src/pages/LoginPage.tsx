import { useMutation, useQueryClient } from "@tanstack/react-query";
import { getRouteApi, useNavigate } from "@tanstack/react-router";
import { useEffect, useState, type FormEvent } from "react";
import { setCsrfToken } from "@/api/client";
import { keys } from "@/api/queries";
import { login } from "@/api/gen/sdk.gen";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { errorMessage } from "@/lib/errors";
import { AuthLayout, FormError } from "./AuthLayout";

const route = getRouteApi("/login");

export function LoginPage() {
  const { redirect } = route.useSearch();
  const navigate = useNavigate();
  const qc = useQueryClient();
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  // The next user of this tab must not see the last one's cached data.
  // Cleared here rather than on the way out, where it would refetch the
  // pages still on screen, whose 401s would send the tab here again.
  useEffect(() => qc.clear(), [qc]);
  const m = useMutation({
    mutationFn: async () => (await login({ body: { username, password } })).data,
    onSuccess: (session) => {
      setCsrfToken(session.csrf_token);
      qc.setQueryData(keys.me, session);
      void navigate({ to: redirect ?? "/" });
    },
  });
  const submit = (e: FormEvent) => {
    e.preventDefault();
    m.mutate();
  };

  return (
    <AuthLayout title="Log in to FleetOps">
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
            autoComplete="current-password"
            required
            value={password}
            onChange={(e) => setPassword(e.target.value)}
          />
        </div>
        <FormError message={m.error ? errorMessage(m.error) : null} />
        <Button type="submit" disabled={m.isPending}>
          {m.isPending ? "Logging in…" : "Log in"}
        </Button>
      </form>
    </AuthLayout>
  );
}
