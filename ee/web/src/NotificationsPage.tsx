import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { BellRing, Mail, Plus, ScrollText, Send, Trash2, Waypoints } from "lucide-react";
import { toast } from "sonner";
import type { DestinationKind, DestinationView, RuleView, Severity, SmtpSecurity, SmtpView } from "@/api/gen/types.gen";
import { envsQuery } from "@/api/queries";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { Page } from "@/components/Page";
import { Panel } from "@/components/Panel";
import { StateBadge } from "@/components/StateBadge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { NativeSelect } from "@/components/ui/native-select";
import { useFeatures } from "@/ee-slots";
import { errorMessage, toastError } from "@/lib/errors";
import { formatAgo } from "@/lib/format";
import { FormDialog } from "@/pages/admin/FormDialog";
import { FrozenNotice } from "./FrozenNotice";
import {
  createDestination,
  createRule,
  deleteDestination,
  deleteRule,
  deliveriesQuery,
  destinationsQuery,
  KIND_LABEL,
  notifyKeys,
  putSmtp,
  rulesQuery,
  SEVERITY_LABEL,
  smtpQuery,
  testDestination,
  testSmtp,
  threshold,
  updateRule,
} from "./notifications";

function useInvalidate() {
  const qc = useQueryClient();
  return () => qc.invalidateQueries({ queryKey: notifyKeys.all });
}

function Field({ id, label, children }: { id: string; label: string; children: React.ReactNode }) {
  return (
    <div className="grid gap-1.5">
      <Label htmlFor={id}>{label}</Label>
      {children}
    </div>
  );
}

const URL_LABEL: Partial<Record<DestinationKind, string>> = {
  slack: "Webhook URL",
  teams: "Webhook URL",
  discord: "Webhook URL",
  webhook: "URL",
};

function DestinationDialog({ onClose }: { onClose: () => void }) {
  const invalidate = useInvalidate();
  const [kind, setKind] = useState<DestinationKind>("slack");
  const [name, setName] = useState("");
  const [url, setUrl] = useState("");
  const [key, setKey] = useState("");
  const [secret, setSecret] = useState("");
  const [recipients, setRecipients] = useState("");
  return (
    <FormDialog
      title="Add destination"
      submitLabel="Add"
      onClose={onClose}
      onSubmit={() =>
        createDestination({
          name,
          kind,
          url: URL_LABEL[kind] ? url : null,
          routing_key: kind === "pagerduty" ? key : null,
          signing_secret: kind === "webhook" && secret ? secret : null,
          recipients: kind === "email" ? recipients.split(/[\s,]+/).filter(Boolean) : null,
        }).then(invalidate)
      }
    >
      <Field id="dest-kind" label="Kind">
        <NativeSelect id="dest-kind" value={kind} onChange={(e) => setKind(e.target.value as DestinationKind)}>
          {(Object.keys(KIND_LABEL) as DestinationKind[]).map((k) => (
            <option key={k} value={k}>
              {KIND_LABEL[k]}
            </option>
          ))}
        </NativeSelect>
      </Field>
      <Field id="dest-name" label="Name">
        <Input id="dest-name" value={name} onChange={(e) => setName(e.target.value)} placeholder="On-call" />
      </Field>
      {URL_LABEL[kind] && (
        <Field id="dest-url" label={URL_LABEL[kind]!}>
          <Input id="dest-url" className="font-ident" value={url} onChange={(e) => setUrl(e.target.value)} autoComplete="off" />
        </Field>
      )}
      {kind === "pagerduty" && (
        <Field id="dest-key" label="Routing key">
          <Input id="dest-key" className="font-ident" value={key} onChange={(e) => setKey(e.target.value)} autoComplete="off" />
        </Field>
      )}
      {kind === "webhook" && (
        <Field id="dest-secret" label="Signing secret (optional)">
          <Input id="dest-secret" type="password" value={secret} onChange={(e) => setSecret(e.target.value)} autoComplete="new-password" />
        </Field>
      )}
      {kind === "email" && (
        <Field id="dest-to" label="Recipients">
          <Input id="dest-to" value={recipients} onChange={(e) => setRecipients(e.target.value)} placeholder="oncall@example.com, security@example.com" />
        </Field>
      )}
    </FormDialog>
  );
}

function RuleDialog({ rule, destinations, onClose }: { rule?: RuleView; destinations: DestinationView[]; onClose: () => void }) {
  const invalidate = useInvalidate();
  const envs = useQuery(envsQuery).data ?? [];
  const [name, setName] = useState(rule?.name ?? "");
  const [scope, setScope] = useState<string[]>(rule?.envs ?? []);
  const [min, setMin] = useState<Severity>(rule?.min_severity ?? "critical");
  const [fix, setFix] = useState(rule?.fixable_only ?? true);
  const [running, setRunning] = useState(rule?.running_only ?? true);
  const [dests, setDests] = useState<number[]>(rule?.destinations ?? []);
  const toggle = <T,>(list: T[], v: T) => (list.includes(v) ? list.filter((x) => x !== v) : [...list, v]);
  const body = { name, enabled: rule?.enabled ?? true, envs: scope, min_severity: min, fixable_only: fix, running_only: running, destinations: dests };
  return (
    <FormDialog
      title={rule ? "Edit rule" : "Add rule"}
      submitLabel={rule ? "Save" : "Add"}
      submitDisabled={!name.trim() || dests.length === 0}
      onClose={onClose}
      onSubmit={() => (rule ? updateRule(rule.id, body) : createRule(body)).then(invalidate)}
    >
      <Field id="rule-name" label="Name">
        <Input id="rule-name" value={name} onChange={(e) => setName(e.target.value)} placeholder="Prod → on-call" />
      </Field>
      <fieldset className="grid gap-1.5">
        <legend className="text-sm font-medium">Environments</legend>
        <p className="text-xs text-meta">None ticked means every environment.</p>
        {envs.map((e) => (
          <label key={e.id} className="flex items-center gap-2 text-sm">
            <Checkbox checked={scope.includes(e.name)} onCheckedChange={() => setScope(toggle(scope, e.name))} />
            {e.name}
          </label>
        ))}
      </fieldset>
      <Field id="rule-min" label="Minimum severity">
        <NativeSelect id="rule-min" value={min} onChange={(e) => setMin(e.target.value as Severity)}>
          {(["critical", "high", "medium", "low"] as Severity[]).map((s) => (
            <option key={s} value={s}>
              {SEVERITY_LABEL[s]}
            </option>
          ))}
        </NativeSelect>
      </Field>
      <label className="flex items-center gap-2 text-sm">
        <Checkbox checked={fix} onCheckedChange={(v) => setFix(v === true)} />
        Only when a fix is available
      </label>
      <label className="flex items-center gap-2 text-sm">
        <Checkbox checked={running} onCheckedChange={(v) => setRunning(v === true)} />
        Only images with running containers
      </label>
      <fieldset className="grid gap-1.5">
        <legend className="text-sm font-medium">Send to</legend>
        {destinations.length === 0 && <p className="text-xs text-meta">Add a destination first.</p>}
        {destinations.map((d) => (
          <label key={d.id} className="flex items-center gap-2 text-sm">
            <Checkbox aria-label={d.name} checked={dests.includes(d.id)} onCheckedChange={() => setDests(toggle(dests, d.id))} />
            {d.name} <span className="text-meta">{KIND_LABEL[d.kind]}</span>
          </label>
        ))}
      </fieldset>
    </FormDialog>
  );
}

function DestinationRow({ d, licensed }: { d: DestinationView; licensed: boolean }) {
  const invalidate = useInvalidate();
  const [deleting, setDeleting] = useState(false);
  const test = useMutation({
    mutationFn: () => testDestination(d.id),
    onSuccess: () => toast.success(`Sent a test to ${d.name}`),
    onError: (e) => toast.error(errorMessage(e)),
  });
  return (
    <tr className="border-t border-border">
      <td className="px-4 py-2 font-medium">{d.name}</td>
      <td className="px-4 py-2 text-ink-muted">{KIND_LABEL[d.kind]}</td>
      <td className="px-4 py-2 font-ident text-xs">{d.kind === "email" ? d.recipients.join(", ") : (d.host ?? "")}</td>
      <td className="px-4 py-2">
        {d.last_error ? (
          <span title={d.last_error_at ? formatAgo(d.last_error_at) : undefined}>
            <StateBadge state={`Failing: ${d.last_error}`} tone="fail" />
          </span>
        ) : (
          <StateBadge state="OK" tone="run" />
        )}
      </td>
      <td className="px-4 py-2 text-right whitespace-nowrap">
        {licensed && (
          <Button size="sm" variant="outline" aria-label={`Send test to ${d.name}`} disabled={test.isPending} onClick={() => test.mutate()}>
            <Send aria-hidden />
            Send test
          </Button>
        )}
        <Button size="sm" variant="ghost" aria-label={`Delete ${d.name}`} onClick={() => setDeleting(true)}>
          <Trash2 aria-hidden />
        </Button>
        <ConfirmDialog
          open={deleting}
          onOpenChange={setDeleting}
          title={`Delete ${d.name}?`}
          description="Rules stop sending here. Rules left with no destination send nothing."
          confirmLabel="Delete"
          destructive
          onConfirm={() => deleteDestination(d.id).then(invalidate)}
        />
      </td>
    </tr>
  );
}

function RuleRow({ r, destinations, licensed }: { r: RuleView; destinations: DestinationView[]; licensed: boolean }) {
  const invalidate = useInvalidate();
  const [editing, setEditing] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const enable = useMutation({
    mutationFn: (enabled: boolean) => updateRule(r.id, { ...r, enabled }),
    onSuccess: invalidate,
    onError: toastError,
  });
  const names = r.destinations.map((id) => destinations.find((d) => d.id === id)?.name).filter(Boolean);
  return (
    <tr className="border-t border-border">
      <td className="px-4 py-2 font-medium">{r.name}</td>
      <td className="px-4 py-2 text-ink-muted">{r.envs.length ? r.envs.join(", ") : "All environments"}</td>
      <td className="px-4 py-2">{threshold(r)}</td>
      <td className="px-4 py-2 text-ink-muted">{names.join(", ")}</td>
      <td className="px-4 py-2">
        <label className="flex items-center gap-2 text-sm">
          <Checkbox checked={r.enabled} disabled={!licensed} onCheckedChange={(v) => enable.mutate(v === true)} aria-label={`${r.name} enabled`} />
          {r.enabled ? "On" : "Off"}
        </label>
      </td>
      <td className="px-4 py-2 text-right whitespace-nowrap">
        {licensed && (
          <Button size="sm" variant="outline" onClick={() => setEditing(true)}>
            Edit
          </Button>
        )}
        <Button size="sm" variant="ghost" aria-label={`Delete rule ${r.name}`} onClick={() => setDeleting(true)}>
          <Trash2 aria-hidden />
        </Button>
        {editing && <RuleDialog rule={r} destinations={destinations} onClose={() => setEditing(false)} />}
        <ConfirmDialog
          open={deleting}
          onOpenChange={setDeleting}
          title={`Delete rule ${r.name}?`}
          description="It stops sending, and forgets what it has already reported."
          confirmLabel="Delete"
          destructive
          onConfirm={() => deleteRule(r.id).then(invalidate)}
        />
      </td>
    </tr>
  );
}

function SmtpPanel({ smtp, licensed }: { smtp: SmtpView | null; licensed: boolean }) {
  const invalidate = useInvalidate();
  const [v, setV] = useState({
    host: smtp?.host ?? "",
    port: String(smtp?.port ?? 587),
    security: (smtp?.security ?? "starttls") as SmtpSecurity,
    username: smtp?.username ?? "",
    password: "",
    from: smtp?.from_address ?? "",
  });
  const [to, setTo] = useState("");
  const save = useMutation({
    mutationFn: () =>
      putSmtp({
        host: v.host,
        port: Number(v.port),
        security: v.security,
        username: v.username || null,
        password: v.password || null,
        from_address: v.from,
      }),
    onSuccess: () => {
      void invalidate();
      setV({ ...v, password: "" });
      toast.success("Email settings saved");
    },
    onError: toastError,
  });
  const test = useMutation({ mutationFn: () => testSmtp(to), onSuccess: () => toast.success(`Sent a test to ${to}`), onError: (e) => toast.error(errorMessage(e)) });
  const set = (k: keyof typeof v) => (e: React.ChangeEvent<HTMLInputElement | HTMLSelectElement>) => setV({ ...v, [k]: e.target.value });
  return (
    <Panel title="Email (SMTP)" icon={Mail}>
      <div className="grid gap-3">
        <p className="text-sm text-ink-muted">Your mail server or relay. For Amazon SES use its SMTP endpoint (email-smtp.&lt;region&gt;.amazonaws.com, port 587, STARTTLS) and SES SMTP credentials.</p>
        <div className="grid gap-3 sm:grid-cols-3">
          <div className="sm:col-span-2">
            <Field id="smtp-host" label="Host">
              <Input id="smtp-host" className="font-ident" value={v.host} onChange={set("host")} disabled={!licensed} />
            </Field>
          </div>
          <Field id="smtp-port" label="Port">
            <Input id="smtp-port" inputMode="numeric" value={v.port} onChange={set("port")} disabled={!licensed} />
          </Field>
        </div>
        <Field id="smtp-security" label="Security">
          <NativeSelect id="smtp-security" value={v.security} onChange={set("security")} disabled={!licensed}>
            <option value="starttls">STARTTLS</option>
            <option value="tls">TLS</option>
            <option value="none">None</option>
          </NativeSelect>
        </Field>
        <div className="grid gap-3 sm:grid-cols-2">
          <Field id="smtp-user" label="Username">
            <Input id="smtp-user" value={v.username} onChange={set("username")} disabled={!licensed} />
          </Field>
          <div className="grid gap-1.5">
            <div className="flex items-center gap-2">
              <Label htmlFor="smtp-password">SMTP password</Label>
              {smtp?.password_set && <span className="rounded-full bg-step px-2 py-0.5 text-[11px] text-ink-muted">Saved</span>}
            </div>
            <Input
              id="smtp-password"
              type="password"
              autoComplete="new-password"
              value={v.password}
              onChange={set("password")}
              placeholder={smtp?.password_set ? "Leave empty to keep it" : ""}
              disabled={!licensed}
            />
          </div>
        </div>
        <Field id="smtp-from" label="From address">
          <Input id="smtp-from" value={v.from} onChange={set("from")} placeholder="fleetops@example.com" disabled={!licensed} />
        </Field>
        {licensed && (
          <div className="flex flex-wrap items-end gap-2">
            <Button disabled={save.isPending} onClick={() => save.mutate()}>
              Save email settings
            </Button>
            <Input aria-label="Send a test email to" className="max-w-56" value={to} onChange={(e) => setTo(e.target.value)} placeholder="you@example.com" />
            <Button variant="outline" disabled={!smtp || !to || test.isPending} onClick={() => test.mutate()}>
              <Send aria-hidden />
              Send test
            </Button>
          </div>
        )}
      </div>
    </Panel>
  );
}

/** Admin → Notifications: where alerts go, which ones, and what was sent. */
export function NotificationsPage() {
  const licensed = useFeatures().includes("notifications");
  const destinations = useQuery(destinationsQuery).data ?? [];
  const rules = useQuery(rulesQuery).data ?? [];
  const smtp = useQuery(smtpQuery);
  const deliveries = useQuery(deliveriesQuery).data ?? [];
  const [adding, setAdding] = useState<"destination" | "rule" | null>(null);
  return (
    <Page title="Notifications" kicker="Enterprise">
      {!licensed && <FrozenNotice>Notifications need a license. Nothing is sent; you can still delete rules and destinations.</FrozenNotice>}
      <div className="grid gap-5">
        <Panel
          title="Rules"
          icon={BellRing}
          flush
          actions={
            licensed && (
              <Button size="sm" onClick={() => setAdding("rule")}>
                <Plus aria-hidden />
                Add rule
              </Button>
            )
          }
        >
          {rules.length === 0 ? (
            <p className="p-4 text-sm text-ink-muted">No rules yet. A rule decides which new CVEs go to which destinations.</p>
          ) : (
            <table aria-label="Rules" className="w-full text-sm">
              <tbody>
                {rules.map((r) => (
                  <RuleRow key={r.id} r={r} destinations={destinations} licensed={licensed} />
                ))}
              </tbody>
            </table>
          )}
        </Panel>
        <Panel
          title="Destinations"
          icon={Waypoints}
          flush
          actions={
            licensed && (
              <Button size="sm" onClick={() => setAdding("destination")}>
                <Plus aria-hidden />
                Add destination
              </Button>
            )
          }
        >
          {destinations.length === 0 ? (
            <p className="p-4 text-sm text-ink-muted">No destinations yet: Slack, email, Microsoft Teams, Discord, PagerDuty or a webhook.</p>
          ) : (
            <table aria-label="Destinations" className="w-full text-sm">
              <tbody>
                {destinations.map((d) => (
                  <DestinationRow key={d.id} d={d} licensed={licensed} />
                ))}
              </tbody>
            </table>
          )}
        </Panel>
        {smtp.isSuccess && <SmtpPanel key={smtp.dataUpdatedAt} smtp={smtp.data} licensed={licensed} />}
        <Panel title="Recent deliveries" icon={ScrollText} flush>
          {deliveries.length === 0 ? (
            <p className="p-4 text-sm text-ink-muted">Nothing sent yet.</p>
          ) : (
            <table aria-label="Recent deliveries" className="w-full text-sm">
              <tbody>
                {deliveries.map((d) => (
                  <tr key={d.id} className="border-t border-border">
                    <td className="px-4 py-2 whitespace-nowrap text-meta">{formatAgo(d.created_at)}</td>
                    <td className="px-4 py-2">{d.title}</td>
                    <td className="px-4 py-2 text-ink-muted">{[d.rule, d.destination].filter(Boolean).join(" → ")}</td>
                    <td className="px-4 py-2" title={d.error ?? undefined}>
                      <StateBadge state={d.status === "sent" ? "Sent" : d.status === "queued" ? "Queued" : "Failed"} tone={d.status === "sent" ? "run" : d.status === "queued" ? "wait" : "fail"} />
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </Panel>
      </div>
      {adding === "destination" && <DestinationDialog onClose={() => setAdding(null)} />}
      {adding === "rule" && <RuleDialog destinations={destinations} onClose={() => setAdding(null)} />}
    </Page>
  );
}
