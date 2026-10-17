import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Check, ExternalLink, FileUp, KeyRound, ReceiptText, ShieldCheck } from "lucide-react";
import { useRef, useState, type CSSProperties } from "react";
import { toast } from "sonner";
import { editionQuery } from "@/api/queries";
import { Page } from "@/components/Page";
import { Panel } from "@/components/Panel";
import { StateBadge, type Tone } from "@/components/StateBadge";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import { formatTime } from "@/lib/format";
import { cn } from "@/lib/utils";
import { GRACE_SECS, licenseQuery, removeLicense, uploadLicense, type LicenseInfo, type LicenseStatus } from "./license";
import { PLANS, PRICING_URL, type Plan } from "./plans";

const TONE: Record<LicenseStatus, Tone> = { valid: "run", grace: "wait", expired: "fail", invalid: "fail", none: "idle" };

const FEATURE_NAMES: Record<string, string> = { audit_export: "Audit export", teams: "Teams", rbac: "Custom roles" };

/** "in 12 days", "today", "3 days ago" for a Unix-seconds time. */
function relDays(unix: number, now: number): string {
  const d = Math.round((unix - now) / 86_400);
  if (d === 0) return "today";
  const n = Math.abs(d);
  const unit = `${n} day${n === 1 ? "" : "s"}`;
  return d > 0 ? `in ${unit}` : `${unit} ago`;
}

/** One line on what the status means for the features. */
function statusLine(l: LicenseInfo, now: number): string {
  const exp = l.expires_at ?? 0;
  switch (l.status) {
    case "valid":
      return "Enterprise features are on.";
    case "grace":
      return `Expired. Enterprise features turn off ${relDays(exp + GRACE_SECS, now)}.`;
    case "expired":
      return "Expired. Enterprise features are off; Community features and your data are unaffected.";
    case "invalid":
      return `Not valid${l.reason ? `: ${l.reason}` : ""}. Enterprise features are off.`;
    default:
      return "No license installed. Everything in Community works; install a license for Enterprise features.";
  }
}

/** "3 of 10", with a hairline track that turns red at the limit. */
function Usage({ used, max }: { used: number; max: number | null }) {
  const share = max ? used / max : undefined;
  return (
    <dd className="grid gap-1.5 tabular-nums">
      <span className="text-lg font-semibold">
        {used} of {max ?? "unlimited"}
      </span>
      {share !== undefined && (
        <span aria-hidden className="h-0.5 overflow-hidden rounded-full bg-muted">
          <span
            className={cn("bar-fill", share >= 1 ? "bg-state-fail" : "bg-foreground/60")}
            style={{ "--share": share } as CSSProperties}
          />
        </span>
      )}
    </dd>
  );
}

function CurrentLicense({ l }: { l: LicenseInfo }) {
  const now = Date.now() / 1000;
  const over = l.max_nodes !== null && l.nodes_in_use > l.max_nodes;
  return (
    <Panel title="Current license" icon={ShieldCheck} aside={l.id ?? undefined}>
      <div className="grid gap-4">
        <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
          <StateBadge state={l.status} tone={TONE[l.status]} />
          <p className="text-sm text-muted-foreground">{statusLine(l, now)}</p>
        </div>
        {l.status !== "none" && (
          <dl aria-label="License details" className="grid gap-4 sm:grid-cols-3 lg:grid-cols-4">
            <div className="grid content-start gap-1">
              <dt className="text-xs font-medium text-muted-foreground">Customer</dt>
              <dd className="truncate text-lg font-semibold">{l.customer ?? "—"}</dd>
            </div>
            <div className="grid content-start gap-1">
              <dt className="text-xs font-medium text-muted-foreground">Expires</dt>
              <dd className="tabular-nums">
                {l.expires_at ? (
                  <>
                    <span className="text-lg font-semibold">{new Date(l.expires_at * 1000).toLocaleDateString()}</span>{" "}
                    <span className="text-xs text-muted-foreground" title={formatTime(l.expires_at)}>
                      {relDays(l.expires_at, now)}
                    </span>
                  </>
                ) : (
                  <span className="text-lg font-semibold">—</span>
                )}
              </dd>
            </div>
            <div className="grid content-start gap-1">
              <dt className="text-xs font-medium text-muted-foreground">Nodes</dt>
              <Usage used={l.nodes_in_use} max={l.max_nodes} />
            </div>
            {l.max_users !== null && (
              <div className="grid content-start gap-1">
                <dt className="text-xs font-medium text-muted-foreground">Users</dt>
                <Usage used={l.users_in_use} max={l.max_users} />
              </div>
            )}
            <div className="col-span-full grid content-start gap-1">
              <dt className="text-xs font-medium text-muted-foreground">Features</dt>
              <dd className="flex flex-wrap gap-1.5">
                {l.features.length ? (
                  l.features.map((f) => (
                    <span key={f} className="inline-flex items-center gap-1 rounded-xs border px-1.5 py-0.5 text-xs">
                      <Check aria-hidden className="size-3 text-state-run-text" />
                      {FEATURE_NAMES[f] ?? f}
                    </span>
                  ))
                ) : (
                  <span className="text-sm text-muted-foreground">none</span>
                )}
              </dd>
            </div>
          </dl>
        )}
        {over && (
          <p role="status" className="text-sm text-state-fail-text">
            Over the node limit. Everything keeps working, but new environments can't be added until you buy another
            block of nodes or remove an environment.
          </p>
        )}
        {l.source === "file" && (
          <p className="text-xs text-muted-foreground">
            Read from the server's <code className="font-mono">--license-file</code>. Installing a license here overrides it.
          </p>
        )}
      </div>
    </Panel>
  );
}

function price(p: Plan, yearly: boolean) {
  if (p.yearly === null || p.monthly === null) return <span className="text-lg font-semibold">Custom</span>;
  return (
    <>
      <span className="text-lg font-semibold">${yearly ? p.yearly : p.monthly}</span>{" "}
      <span className="text-xs text-muted-foreground">{yearly ? "/ year" : "/ month"} per 10 nodes</span>
    </>
  );
}

function Plans({ current }: { current: boolean }) {
  const [yearly, setYearly] = useState(true);
  return (
    <Panel
      title={current ? "Renew or change plan" : "Plans"}
      icon={ReceiptText}
      aside="USD before tax, sold in blocks of ten nodes"
      actions={
        <div role="group" aria-label="Billing period" className="flex rounded-md border p-0.5 text-xs">
          {[true, false].map((y) => (
            <button
              key={String(y)}
              type="button"
              aria-pressed={yearly === y}
              onClick={() => setYearly(y)}
              className={cn(
                "rounded-sm px-2 py-1 font-medium transition-colors",
                yearly === y ? "bg-accent text-foreground" : "text-muted-foreground hover:text-foreground",
              )}
            >
              {y ? "Yearly" : "Monthly"}
            </button>
          ))}
        </div>
      }
    >
      <div className="grid gap-3 md:grid-cols-3">
        {PLANS.map((p) => (
          <article
            key={p.name}
            aria-label={`${p.name} plan`}
            className={cn("grid content-start gap-3 rounded-md border p-3", p.recommended && "border-primary bg-primary/5")}
          >
            <header className="grid gap-0.5">
              <div className="flex items-center gap-2">
                <h3 className="text-sm font-semibold">{p.name}</h3>
                {p.recommended && (
                  <span className="rounded-xs bg-primary px-1.5 text-[11px] leading-4 font-medium text-primary-foreground">
                    Recommended
                  </span>
                )}
              </div>
              <p className="tabular-nums">{price(p, yearly)}</p>
              <p className="text-xs text-muted-foreground">{p.nodes}</p>
            </header>
            <ul className="grid gap-1.5 text-sm">
              {p.features.map((f) => (
                <li key={f} className="flex gap-2">
                  <Check aria-hidden className="mt-0.5 size-3.5 shrink-0 text-muted-foreground" />
                  {f}
                </li>
              ))}
            </ul>
            <p className="text-xs text-muted-foreground">Support: {p.support}</p>
            <Button asChild size="sm" variant={p.recommended ? "default" : "outline"} className="mt-auto">
              <a href={p.cta.href} target="_blank" rel="noreferrer">
                {p.cta.label}
                <ExternalLink aria-hidden />
              </a>
            </Button>
          </article>
        ))}
      </div>
      <p className="mt-3 text-xs text-muted-foreground">
        Nodes are Docker or Podman hosts and Kubernetes cluster nodes; users, containers and pods aren't counted. Licenses are
        checked offline, so air-gapped installs work.{" "}
        <a href={PRICING_URL} target="_blank" rel="noreferrer" className="underline underline-offset-2">
          Full pricing and FAQ
        </a>
      </p>
    </Panel>
  );
}

export function LicensePage() {
  const qc = useQueryClient();
  const license = useQuery(licenseQuery);
  const [text, setText] = useState("");
  const fileInput = useRef<HTMLInputElement>(null);
  const done = (l: LicenseInfo, message: string) => {
    qc.setQueryData(licenseQuery.queryKey, l);
    void qc.invalidateQueries({ queryKey: editionQuery.queryKey });
    toast.success(message);
  };
  const upload = useMutation({
    mutationFn: uploadLicense,
    onSuccess: (l) => {
      setText("");
      done(l, "License installed");
    },
    onError: (e) => toast.error(e.message),
  });
  const remove = useMutation({
    mutationFn: removeLicense,
    onSuccess: (l) => done(l, "License removed"),
    onError: (e) => toast.error(e.message),
  });
  const l = license.data;
  const active = l?.status === "valid";
  return (
    <Page title="License" meta="Enterprise features are unlocked by a signed license file, checked offline.">
      {l && <CurrentLicense l={l} />}
      <Panel
        title={l && l.status !== "none" ? "Replace license" : "Install license"}
        icon={KeyRound}
        actions={
          <>
            <input
              ref={fileInput}
              type="file"
              accept=".lic,text/plain"
              className="hidden"
              onChange={async (e) => {
                const f = e.target.files?.[0];
                e.target.value = "";
                if (f) setText((await f.text()).trim());
              }}
            />
            <Button type="button" variant="ghost" size="sm" onClick={() => fileInput.current?.click()}>
              <FileUp aria-hidden />
              Load .lic file
            </Button>
          </>
        }
      >
        <form
          className="grid gap-2"
          onSubmit={(e) => {
            e.preventDefault();
            upload.mutate(text.trim());
          }}
        >
          <label htmlFor="license-text" className="text-sm font-medium">
            License key
          </label>
          <Textarea
            id="license-text"
            rows={4}
            placeholder="Paste the license you received after purchase"
            className="font-mono text-xs"
            value={text}
            onChange={(e) => setText(e.target.value)}
          />
          <div className="flex flex-wrap gap-2">
            <Button type="submit" disabled={!text.trim() || upload.isPending}>
              Install license
            </Button>
            {l?.source === "uploaded" && (
              <Button type="button" variant="outline" disabled={remove.isPending} onClick={() => remove.mutate()}>
                Remove license
              </Button>
            )}
          </div>
        </form>
      </Panel>
      {l && <Plans current={active} />}
    </Page>
  );
}
