import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { toast } from "sonner";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { columns, DataTable, type Column } from "@/components/DataTable";
import { Page } from "@/components/Page";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { NativeSelect } from "@/components/ui/native-select";
import { Textarea } from "@/components/ui/textarea";
import { useFeatures } from "@/ee-slots";
import { errorMessage } from "@/lib/errors";
import { FormDialog } from "@/pages/admin/FormDialog";
import {
  blankSettings,
  connectionsQuery,
  createConnection,
  createMapping,
  deleteConnection,
  deleteMapping,
  KIND_LABEL,
  LDAP_PRESETS,
  PRESET_LABEL,
  list,
  mappingsQuery,
  putSsoSettings,
  ssoKeys,
  ssoSettingsQuery,
  testConnection,
  updateConnection,
  type LdapPreset,
  type PasswordLogin,
  type SsoConnection,
  type SsoKind,
  type SsoSecrets,
  type SsoSettings,
  type TestResult,
} from "./sso";
import { teamsQuery } from "./teams";

type Dialog =
  | { kind: "new" }
  | { kind: "edit" | "mappings" | "test" | "delete"; conn: SsoConnection }
  | null;

const col = columns<SsoConnection>();

/** Admin → Single sign-on: identity provider connections and group mappings. */
export function SsoPage() {
  const frozen = !useFeatures().includes("sso");
  const qc = useQueryClient();
  const conns = useQuery(connectionsQuery);
  const [dialog, setDialog] = useState<Dialog>(null);
  const close = () => setDialog(null);
  const refresh = () => qc.invalidateQueries({ queryKey: ssoKeys.all });
  const toggle = useMutation({
    mutationFn: (c: SsoConnection) => updateConnection(c.id, { enabled: !c.enabled }),
    onSuccess: (c) => {
      toast.success(`${c.name} ${c.enabled ? "enabled" : "disabled"}`);
      return refresh();
    },
    onError: (e) => toast.error(errorMessage(e)),
  });
  const cols: Column<SsoConnection>[] = [
    col.accessor("name", { header: "Name", cell: (c) => <span className="font-medium">{c.getValue()}</span> }),
    col.accessor("kind", { header: "Provider", cell: (c) => KIND_LABEL[c.row.original.kind] }),
    col.accessor("enabled", {
      header: "Status",
      cell: (c) => (c.getValue() ? "Enabled" : <span className="text-muted-foreground">Disabled</span>),
    }),
    col.accessor("user_count", { header: "Users" }),
    col.display({
      id: "actions",
      header: () => <span className="sr-only">Actions</span>,
      cell: (c) => {
        const conn = c.row.original;
        return (
          <span className="flex justify-end gap-2">
            <Button
              variant="outline"
              size="sm"
              disabled={toggle.isPending || (frozen && !conn.enabled)}
              onClick={() => toggle.mutate(conn)}
            >
              {conn.enabled ? "Disable" : "Enable"}
            </Button>
            <Button variant="outline" size="sm" onClick={() => setDialog({ kind: "mappings", conn })}>
              Mappings
            </Button>
            <Button variant="outline" size="sm" onClick={() => setDialog({ kind: "test", conn })}>
              Test
            </Button>
            <Button variant="outline" size="sm" disabled={frozen} onClick={() => setDialog({ kind: "edit", conn })}>
              Edit
            </Button>
            <Button variant="outline" size="sm" onClick={() => setDialog({ kind: "delete", conn })}>
              Delete
            </Button>
          </span>
        );
      },
    }),
  ];
  return (
    <Page
      title="Single sign-on"
      meta="People sign in with their company account. Their groups decide their teams."
      actions={
        <Button disabled={frozen} onClick={() => setDialog({ kind: "new" })}>
          New connection
        </Button>
      }
    >
      <div className="grid gap-6">
        {frozen && (
          <p className="text-sm text-muted-foreground">
            Without a license, existing users keep signing in. You can disable or remove connections and mappings, not add them.
          </p>
        )}
        <DataTable
          label="Connections"
          data={conns.data}
          error={conns.error}
          columns={cols}
          getRowId={(c) => String(c.id)}
          empty="No connections yet."
          initialSorting={[{ id: "name", desc: false }]}
        />
        <PasswordLoginSetting frozen={frozen} />
      </div>
      {dialog?.kind === "new" && (
        <ConnectionDialog hasLdap={conns.data?.some((c) => c.kind === "ldap") ?? false} onClose={close} onDone={refresh} />
      )}
      {dialog?.kind === "edit" && <ConnectionDialog conn={dialog.conn} onClose={close} onDone={refresh} />}
      {dialog?.kind === "mappings" && <MappingsDialog conn={dialog.conn} frozen={frozen} onClose={close} />}
      {dialog?.kind === "test" && <TestDialog conn={dialog.conn} onClose={close} />}
      {dialog?.kind === "delete" && (
        <ConfirmDialog
          open
          onOpenChange={(open) => !open && close()}
          title={`Delete ${dialog.conn.name}?`}
          description={
            dialog.conn.user_count > 0
              ? `Its ${dialog.conn.user_count} user(s) are logged out and lose the teams their groups gave them. Their accounts stay, but can't sign in; delete them on the Users page.`
              : "Nobody has signed in through it yet."
          }
          confirmLabel="Delete"
          destructive
          onConfirm={async () => {
            await deleteConnection(dialog.conn.id);
            toast.success(`Deleted ${dialog.conn.name}`);
            await refresh();
          }}
        />
      )}
    </Page>
  );
}

function PasswordLoginSetting({ frozen }: { frozen: boolean }) {
  const qc = useQueryClient();
  const settings = useQuery(ssoSettingsQuery);
  // The choice shows at once; a refusal puts the saved value back.
  const [picked, setPicked] = useState<PasswordLogin | null>(null);
  const save = useMutation({
    mutationFn: putSsoSettings,
    onError: (e) => {
      setPicked(null);
      toast.error(errorMessage(e));
    },
    onSettled: () => qc.invalidateQueries({ queryKey: ssoKeys.settings }),
  });
  const value = picked ?? settings.data?.password_login;
  const option = (v: PasswordLogin, label: string, hint: string) => (
    <label className="flex items-start gap-2 text-sm">
      <input
        type="radio"
        name="password-login"
        className="mt-1"
        checked={value === v}
        disabled={save.isPending || (frozen && v === "local_admins")}
        onChange={() => {
          setPicked(v);
          save.mutate(v);
        }}
      />
      <span>
        {label}
        <span className="block text-xs text-muted-foreground">{hint}</span>
      </span>
    </label>
  );
  return (
    <fieldset className="grid gap-2 rounded-md border p-4">
      <legend className="px-1 text-sm font-medium">Allow password login</legend>
      {option("everyone", "Everyone", "Local accounts keep their passwords.")}
      {option(
        "local_admins",
        "Local admins only",
        "Everyone else signs in through a connection. Local admins stay as the way in if single sign-on breaks.",
      )}
    </fieldset>
  );
}

function Field(props: { id: string; label: string; hint?: string; children: React.ReactNode }) {
  return (
    <div className="grid gap-2">
      <Label htmlFor={props.id}>{props.label}</Label>
      {props.children}
      {props.hint && <p className="text-xs text-muted-foreground">{props.hint}</p>}
    </div>
  );
}

/** Creates a connection, or edits `conn`. Secrets are never shown; empty keeps the saved one. */
export function ConnectionDialog({
  conn,
  hasLdap = false,
  onClose,
  onDone,
}: {
  conn?: SsoConnection;
  /** One LDAP connection is the limit; hide the kind once it exists. */
  hasLdap?: boolean;
  onClose: () => void;
  onDone: () => Promise<unknown>;
}) {
  const [name, setName] = useState(conn?.name ?? "");
  const [settings, setSettings] = useState<SsoSettings>(conn?.settings ?? blankSettings("oidc"));
  const [secrets, setSecrets] = useState<SsoSecrets>({});
  const [requireMapped, setRequireMapped] = useState(conn?.require_mapped_group ?? false);
  const [hours, setHours] = useState(String(conn?.session_hours ?? 12));
  // List fields keep what was typed; they are split on save.
  const initial = conn?.settings;
  const [domains, setDomains] = useState(initial?.kind === "google" ? initial.hosted_domains.join(", ") : "");
  const [orgs, setOrgs] = useState(initial?.kind === "github" ? initial.orgs.join(", ") : "");
  const [scopes, setScopes] = useState(initial?.kind === "oidc" ? initial.scopes.join(" ") : "openid profile email");
  const saved = (s: string) => conn?.secrets_set.includes(s) ?? false;
  const set = (patch: Partial<SsoSettings>) => setSettings((s) => ({ ...s, ...patch }) as SsoSettings);
  const secretField = (key: keyof SsoSecrets, label: string, multiline = false) => {
    const props = {
      id: `sso-${key}`,
      value: secrets[key] ?? "",
      placeholder: saved(key) ? "Saved — leave empty to keep" : "",
      autoComplete: "off",
      onChange: (e: React.ChangeEvent<HTMLInputElement | HTMLTextAreaElement>) =>
        setSecrets((s) => ({ ...s, [key]: e.target.value })),
    };
    return (
      <Field id={`sso-${key}`} label={label}>
        {multiline ? <Textarea rows={4} {...props} /> : <Input type="password" {...props} />}
      </Field>
    );
  };
  return (
    <FormDialog
      title={conn ? `Edit ${conn.name}` : "New connection"}
      submitLabel={conn ? "Save" : "Add"}
      onClose={onClose}
      onSubmit={async () => {
        const sent: SsoSecrets = Object.fromEntries(Object.entries(secrets).filter(([, v]) => v)) as SsoSecrets;
        const lists: SsoSettings =
          settings.kind === "google"
            ? { ...settings, hosted_domains: list(domains) }
            : settings.kind === "github"
              ? { ...settings, orgs: list(orgs) }
              : settings.kind === "oidc"
                ? { ...settings, scopes: scopes.split(/\s+/).filter(Boolean) }
                : settings;
        const body = {
          name: name.trim(),
          settings: lists,
          secrets: sent,
          require_mapped_group: requireMapped,
          session_hours: Number(hours),
        };
        if (conn) await updateConnection(conn.id, body);
        else await createConnection(body);
        toast.success(conn ? `Saved ${body.name}` : `Added ${body.name}; map its groups, then enable it`);
        await onDone();
      }}
    >
      {!conn && (
        <Field id="sso-kind" label="Provider">
          <NativeSelect id="sso-kind" value={settings.kind} onChange={(e) => setSettings(blankSettings(e.target.value as SsoKind))}>
            {(Object.keys(KIND_LABEL) as SsoKind[]).filter((k) => k !== "ldap" || !hasLdap).map((k) => (
              <option key={k} value={k}>
                {KIND_LABEL[k]}
              </option>
            ))}
          </NativeSelect>
        </Field>
      )}
      <Field
        id="sso-name"
        label="Name"
        hint={settings.kind === "ldap" ? "Shown on the login page as “Use your … account”." : "Shown on the login page as “Sign in with …”."}
      >
        <Input id="sso-name" required value={name} onChange={(e) => setName(e.target.value)} />
      </Field>
      {settings.kind === "microsoft" && (
        <Field id="sso-tenant" label="Tenant ID or domain">
          <Input id="sso-tenant" required value={settings.tenant} onChange={(e) => set({ tenant: e.target.value })} />
        </Field>
      )}
      {settings.kind === "oidc" && (
        <>
          <Field id="sso-issuer" label="Issuer URL" hint="FleetOps reads /.well-known/openid-configuration under it.">
            <Input id="sso-issuer" required value={settings.issuer} onChange={(e) => set({ issuer: e.target.value })} />
          </Field>
          <Field id="sso-groups-claim" label="Groups claim">
            <Input id="sso-groups-claim" value={settings.groups_claim} onChange={(e) => set({ groups_claim: e.target.value })} />
          </Field>
          <Field id="sso-scopes" label="Scopes">
            <Input id="sso-scopes" value={scopes} onChange={(e) => setScopes(e.target.value)} />
          </Field>
        </>
      )}
      {settings.kind !== "ldap" && (
        <>
          <Field id="sso-client-id" label="Client ID">
            <Input id="sso-client-id" required value={settings.client_id} onChange={(e) => set({ client_id: e.target.value })} />
          </Field>
          {secretField("client_secret", "Client secret")}
        </>
      )}
      {settings.kind === "ldap" && <LdapFields settings={settings} set={set} secretField={secretField} />}
      {settings.kind === "google" && (
        <>
          <Field id="sso-domains" label="Allowed domains" hint="Comma-separated Workspace domains whose accounts may sign in.">
            <Input id="sso-domains" required value={domains} onChange={(e) => setDomains(e.target.value)} />
          </Field>
          <Field id="sso-admin-email" label="Workspace admin email" hint="To read groups: the service account acts as this admin.">
            <Input id="sso-admin-email" value={settings.admin_email ?? ""} onChange={(e) => set({ admin_email: e.target.value || null })} />
          </Field>
          {secretField("service_account_json", "Service account key (JSON)", true)}
        </>
      )}
      {settings.kind === "github" && (
        <>
          <Field id="sso-orgs" label="Allowed organisations" hint="Comma-separated. Teams in them become groups as org/team.">
            <Input id="sso-orgs" required value={orgs} onChange={(e) => setOrgs(e.target.value)} />
          </Field>
          <Field id="sso-base-url" label="GitHub Enterprise URL" hint="Leave empty for github.com.">
            <Input id="sso-base-url" value={settings.base_url ?? ""} onChange={(e) => set({ base_url: e.target.value || null })} />
          </Field>
        </>
      )}
      {conn && conn.kind !== "ldap" && (
        <Field id="sso-callback" label="Callback URL" hint="Register this redirect URI with the provider.">
          <Input
            id="sso-callback"
            readOnly
            value={conn.callback_url ?? "Set FLEETOPS_PUBLIC_URL to get the callback URL"}
            onFocus={(e) => e.target.select()}
          />
        </Field>
      )}
      <Field id="sso-hours" label="Session length (hours)" hint="How long a sign-in lasts before the provider is asked again.">
        <Input id="sso-hours" type="number" min={1} max={720} value={hours} onChange={(e) => setHours(e.target.value)} />
      </Field>
      <label className="flex items-center gap-2 text-sm">
        <Checkbox id="sso-require" checked={requireMapped} onCheckedChange={(v) => setRequireMapped(!!v)} />
        Only people in a mapped group may sign in
      </label>
    </FormDialog>
  );
}

type LdapSettings = Extract<SsoSettings, { kind: "ldap" }>;

function LdapFields({
  settings,
  set,
  secretField,
}: {
  settings: LdapSettings;
  set: (patch: Partial<LdapSettings>) => void;
  secretField: (key: keyof SsoSecrets, label: string) => React.ReactNode;
}) {
  const text = (key: keyof LdapSettings, label: string, opts: { hint?: string; required?: boolean; nullable?: boolean } = {}) => (
    <Field id={`sso-${key}`} label={label} hint={opts.hint}>
      <Input
        id={`sso-${key}`}
        required={opts.required}
        value={(settings[key] as string | null | undefined) ?? ""}
        onChange={(e) => set({ [key]: opts.nullable ? e.target.value || null : e.target.value })}
      />
    </Field>
  );
  return (
    <>
      <Field id="sso-preset" label="Directory">
        <NativeSelect
          id="sso-preset"
          value={settings.preset}
          onChange={(e) => {
            const preset = e.target.value as LdapPreset;
            set({ preset, ...LDAP_PRESETS[preset] });
          }}
        >
          {(Object.keys(PRESET_LABEL) as LdapPreset[]).map((p) => (
            <option key={p} value={p}>
              {PRESET_LABEL[p]}
            </option>
          ))}
        </NativeSelect>
      </Field>
      {text("url", "Server URL", { required: true, hint: "ldap://dc.acme.io or ldaps://dc.acme.io:636" })}
      <label className="flex items-center gap-2 text-sm">
        <Checkbox id="sso-starttls" checked={settings.starttls} onCheckedChange={(v) => set({ starttls: !!v })} />
        Use StartTLS (ldap:// only)
      </label>
      <Field id="sso-ca_cert" label="CA certificate (PEM, optional)" hint="Trust this CA instead of the system's.">
        <Textarea id="sso-ca_cert" rows={3} value={settings.ca_cert ?? ""} onChange={(e) => set({ ca_cert: e.target.value || null })} />
      </Field>
      <label className="flex items-center gap-2 text-sm">
        <Checkbox id="sso-skip-verify" checked={settings.skip_tls_verify} onCheckedChange={(v) => set({ skip_tls_verify: !!v })} />
        Skip TLS verification
      </label>
      {settings.skip_tls_verify && (
        <p role="alert" className="text-xs text-destructive">
          The server's certificate is not verified: anyone on the network path can read passwords. Use a CA certificate instead.
        </p>
      )}
      {text("bind_dn", "Bind DN", { nullable: true, hint: "The service account that searches. Empty searches anonymously." })}
      {secretField("bind_password", "Bind password")}
      {text("user_base_dn", "User base DN", { required: true, hint: "e.g. ou=people,dc=acme,dc=io" })}
      {text("user_filter", "User filter", { required: settings.preset === "custom", hint: "{username} is what the person types." })}
      {text("username_attr", "Username attribute", { required: settings.preset === "custom" })}
      {text("email_attr", "Email attribute")}
      {text("group_base_dn", "Group base DN", { nullable: true, hint: "Empty reads no groups." })}
      {text("group_filter", "Group filter", { hint: "{user_dn} and {username} are filled in. Groups map by their DN." })}
    </>
  );
}

const ADMIN_TARGET = "admin";

export function MappingsDialog({ conn, frozen, onClose }: { conn: SsoConnection; frozen: boolean; onClose: () => void }) {
  const qc = useQueryClient();
  const mappings = useQuery(mappingsQuery(conn.id));
  const teams = useQuery(teamsQuery);
  const [group, setGroup] = useState("");
  const [target, setTarget] = useState("");
  const refresh = () => qc.invalidateQueries({ queryKey: ssoKeys.mappings(conn.id) });
  const add = useMutation({
    mutationFn: () =>
      createMapping(conn.id, group.trim(), target === ADMIN_TARGET ? { admin: true } : { team_id: Number(target) }),
    onSuccess: async () => {
      setGroup("");
      await refresh();
    },
    meta: { inline: true },
  });
  const remove = useMutation({ mutationFn: (id: number) => deleteMapping(conn.id, id), onSuccess: refresh });
  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Group mappings: {conn.name}</DialogTitle>
          <DialogDescription>
            Members of a group join its team, or become FleetOps admins, when they sign in. Removing a mapping takes it away now.
          </DialogDescription>
        </DialogHeader>
        <form
          className="flex flex-wrap items-end gap-2"
          onSubmit={(e) => {
            e.preventDefault();
            add.mutate();
          }}
        >
          <div className="grid flex-1 gap-1">
            <Label htmlFor="map-group">Group</Label>
            <Input id="map-group" required disabled={frozen} value={group} onChange={(e) => setGroup(e.target.value)} />
          </div>
          <div className="grid gap-1">
            <Label htmlFor="map-target">Gives</Label>
            <NativeSelect id="map-target" required disabled={frozen} value={target} onChange={(e) => setTarget(e.target.value)}>
              <option value="" disabled>
                Choose…
              </option>
              {(teams.data ?? []).map((t) => (
                <option key={t.id} value={t.id}>
                  Team {t.name}
                </option>
              ))}
              <option value={ADMIN_TARGET}>FleetOps admin</option>
            </NativeSelect>
          </div>
          <Button type="submit" disabled={frozen || add.isPending || !group.trim() || !target}>
            Add
          </Button>
        </form>
        {add.error && (
          <p role="alert" className="text-sm text-destructive">
            {errorMessage(add.error)}
          </p>
        )}
        {mappings.data?.length === 0 && <p className="text-sm text-muted-foreground">No mappings yet.</p>}
        <ul className="grid gap-1">
          {(mappings.data ?? []).map((m) => (
            <li key={m.id} className="flex items-center justify-between rounded-md border px-3 py-1.5 text-sm">
              <span>
                <code>{m.group}</code> → {m.admin ? "FleetOps admin" : `team ${m.team_name}`}
              </span>
              <Button variant="outline" size="sm" aria-label={`Remove ${m.group}`} disabled={remove.isPending} onClick={() => remove.mutate(m.id)}>
                Remove
              </Button>
            </li>
          ))}
        </ul>
      </DialogContent>
    </Dialog>
  );
}

function TestDialog({ conn, onClose }: { conn: SsoConnection; onClose: () => void }) {
  const [who, setWho] = useState("");
  const [result, setResult] = useState<TestResult | null>(null);
  const key = conn.kind === "ldap" ? "username" : "email";
  const run = useMutation({
    mutationFn: () => testConnection(conn.id, who.trim() ? { [key]: who.trim() } : {}),
    onSuccess: setResult,
    meta: { inline: true },
  });
  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Test {conn.name}</DialogTitle>
          <DialogDescription>Checks that FleetOps can reach the provider. It never asks for anyone's password.</DialogDescription>
        </DialogHeader>
        <form
          className="grid gap-3"
          onSubmit={(e) => {
            e.preventDefault();
            run.mutate();
          }}
        >
          {conn.kind === "google" && (
            <Field id="test-email" label="List groups for (optional)">
              <Input id="test-email" type="email" value={who} onChange={(e) => setWho(e.target.value)} />
            </Field>
          )}
          {conn.kind === "ldap" && (
            <Field id="test-username" label="Look up username (optional)">
              <Input id="test-username" value={who} onChange={(e) => setWho(e.target.value)} />
            </Field>
          )}
          <Button type="submit" disabled={run.isPending}>
            {run.isPending ? "Testing…" : "Run test"}
          </Button>
        </form>
        {run.error && (
          <p role="alert" className="text-sm text-destructive">
            {errorMessage(run.error)}
          </p>
        )}
        {result && (
          <div role="status" className={result.ok ? "text-sm" : "text-sm text-destructive"}>
            {result.ok ? "OK: " : "Failed: "}
            {result.detail}
            {result.groups && result.groups.length > 0 && (
              <ul className="mt-2 list-disc pl-5 text-muted-foreground">
                {result.groups.map((g) => (
                  <li key={g}>{g}</li>
                ))}
              </ul>
            )}
          </div>
        )}
      </DialogContent>
    </Dialog>
  );
}
