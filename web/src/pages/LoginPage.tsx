import { useMutation, useQueryClient } from "@tanstack/react-query";
import { getRouteApi, useNavigate } from "@tanstack/react-router";
import { useEffect, useState, type FormEvent } from "react";
import { setCsrfToken } from "@/api/client";
import { keys } from "@/api/queries";
import { getAuthProviders, login } from "@/api/gen/sdk.gen";
import type { LoginProvider } from "@/api/gen/types.gen";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { errorMessage } from "@/lib/errors";
import { AuthLayout, FormError } from "./AuthLayout";

const route = getRouteApi("/login");

/** Why a single sign-on attempt came back to this page. */
export const SSO_ERRORS = ["denied", "not_allowed", "seat_limit", "unlicensed", "disabled", "invalid"] as const;
export type SsoError = (typeof SSO_ERRORS)[number];

export const SSO_ERROR_TEXT: Record<SsoError, string> = {
  denied: "Your account isn't allowed to sign in to FleetOps. Ask an admin to map one of your groups.",
  not_allowed: "That account isn't from an allowed organisation or domain.",
  seat_limit: "FleetOps has reached its licensed user limit. Ask an admin.",
  unlicensed: "New sign-ins are paused until the Enterprise license is renewed.",
  disabled: "That sign-in option is turned off.",
  invalid: "Sign-in failed. Try again, or ask an admin to check the SSO settings.",
};

/** Enabled single sign-on providers (null while loading); none in Community or when the server can't say. */
function useProviders(): LoginProvider[] | null {
  const [providers, setProviders] = useState<LoginProvider[] | null>(null);
  useEffect(() => {
    let live = true;
    getAuthProviders()
      .then((r) => live && setProviders(r.data ?? []))
      .catch(() => live && setProviders([]));
    return () => {
      live = false;
    };
  }, []);
  return providers;
}

export function LoginPage() {
  const { redirect, sso_error } = route.useSearch();
  const navigate = useNavigate();
  const qc = useQueryClient();
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const providers = useProviders();
  const [showPassword, setShowPassword] = useState(false);
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
  const returnTo = encodeURIComponent(redirect ?? "/");
  // An LDAP directory signs in on the password form, not through a button.
  const buttons = providers?.filter((p) => p.kind !== "ldap") ?? null;
  const directory = providers?.find((p) => p.kind === "ldap");
  // Nothing until the providers are known, so the form doesn't flash and vanish.
  const passwordForm = buttons !== null && (buttons.length === 0 || !!directory || showPassword);

  return (
    <AuthLayout title="Log in to FleetOps">
      <div className="grid gap-4">
        {sso_error && (
          <p role="alert" className="text-sm text-destructive">
            {SSO_ERROR_TEXT[sso_error]}
          </p>
        )}
        {buttons && buttons.length > 0 && (
          <div className="grid gap-2">
            {buttons.map((p) => (
              <Button key={p.id} variant="outline" asChild>
                <a href={`/api/v1/ee/sso/${p.id}/start?return_to=${returnTo}`}>Sign in with {p.name}</a>
              </Button>
            ))}
          </div>
        )}
        {buttons && !passwordForm && (
          <button
            type="button"
            className="justify-self-center text-sm text-muted-foreground underline-offset-4 hover:underline"
            onClick={() => setShowPassword(true)}
          >
            Sign in with password
          </button>
        )}
        {passwordForm && (
          <form onSubmit={submit} className="grid gap-4">
            {directory && <p className="text-sm text-muted-foreground">Use your {directory.name} account.</p>}
            <div className="grid gap-2">
              <Label htmlFor="username">Username</Label>
              <Input
                id="username"
                autoComplete="username"
                required
                autoFocus={showPassword}
                value={username}
                onChange={(e) => setUsername(e.target.value)}
              />
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
        )}
      </div>
    </AuthLayout>
  );
}
