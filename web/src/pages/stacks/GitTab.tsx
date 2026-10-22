import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { toast } from "sonner";
import {
  detachStackGit,
  resetStackGitHostKey,
  resumeStackGit,
  rotateStackGitSecret,
  setStackGit,
  syncStackGit,
} from "@/api/gen/sdk.gen";
import type { DeploySource, SyncOutcome } from "@/api/gen/types.gen";
import { keys, stackGitQuery } from "@/api/queries";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { formatAgo } from "@/lib/format";
import { errorMessage } from "@/lib/errors";
import { FormError } from "@/pages/AuthLayout";
import { GitSettingsForm, gitBody, gitFormFrom, gitFormValid, type GitForm } from "./GitSettingsForm";

export const short = (sha: string | null | undefined) => (sha ? sha.slice(0, 7) : "");

/** The webhook URL as the git host must call it. */
export const absoluteUrl = (url: string) => (url.startsWith("/") ? `${window.location.origin}${url}` : url);

/** Hosts whose published SSH host keys FleetOps ships. */
const PUBLISHED_HOST_KEYS = ["github.com", "gitlab.com", "bitbucket.org"];

/** The host of an ssh URL (`ssh://…` or `[user@]host:path`). */
export function sshHost(url: string): string {
  const rest = url.startsWith("ssh://") ? url.slice(6).split("/")[0] : url.split(":")[0];
  return rest.split("@").pop()!.split(":")[0].toLowerCase();
}

/** A sync that failed on a changed host key, which an editor may accept. */
const hostKeyChanged = (error: string | null) =>
  !!error && error.startsWith("the host key of ") && error.includes(" changed (now ");

/** Where a freshly created secret waits for the stack page to show it. */
export const secretKey = (env: string, stack: string) => ["stack-git-secret", env, stack] as const;

function outcomeText(o: SyncOutcome): string {
  switch (o.outcome) {
    case "up_to_date":
      return `Up to date at ${short(o.commit)}`;
    case "pending":
      return `Update available: ${short(o.commit)}`;
    case "deploying":
      return `Deploying ${short(o.commit)} as revision #${o.revision}`;
    case "queued":
      return "A sync is already running; it fetches again when it finishes";
    case "failed":
      return o.error;
  }
}

async function copy(text: string) {
  await navigator.clipboard.writeText(text);
  toast.success("Copied");
}

/** Shows a webhook secret once, with where to put it. */
export function WebhookSecretDialog({ url, secret, onClose }: { url: string; secret: string; onClose: () => void }) {
  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Webhook secret</DialogTitle>
          <DialogDescription>Shown once. Copy it into your git host&apos;s webhook settings.</DialogDescription>
        </DialogHeader>
        <div className="grid gap-1">
          <span className="text-sm font-medium">Payload URL</span>
          <pre aria-label="Webhook URL" className="overflow-x-auto rounded-md bg-muted p-2 font-mono text-xs">
            {absoluteUrl(url)}
          </pre>
          <span className="text-sm font-medium">Secret</span>
          <pre aria-label="Webhook secret" className="overflow-x-auto rounded-md bg-muted p-2 font-mono text-xs">
            {secret}
          </pre>
        </div>
        <ul className="grid gap-0.5 text-sm text-muted-foreground">
          <li>GitHub: content type application/json, and the secret as Secret.</li>
          <li>GitLab: the secret as Secret token.</li>
          <li>Gitea and Forgejo: the secret as Secret.</li>
          <li>
            Other senders: add <span className="font-ident">?secret=</span> and the secret to the URL.
          </li>
        </ul>
        <DialogFooter>
          <Button variant="outline" onClick={() => void copy(absoluteUrl(url))}>
            Copy URL
          </Button>
          <Button variant="outline" onClick={() => void copy(secret)}>
            Copy secret
          </Button>
          <Button onClick={onClose}>Done</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

/** A git stack's source, its last sync and what is waiting to deploy. */
export function GitTab({
  env,
  stack,
  canEdit,
  canOperate,
  onReview,
}: {
  env: string;
  stack: string;
  canEdit: boolean;
  canOperate: boolean;
  onReview: (source: DeploySource) => void;
}) {
  const qc = useQueryClient();
  const git = useQuery(stackGitQuery(env, stack));
  const [form, setForm] = useState<GitForm | null>(null);
  const [secret, setSecret] = useState<string | null>(() => {
    const s = qc.getQueryData<string>(secretKey(env, stack)) ?? null;
    if (s) qc.removeQueries({ queryKey: secretKey(env, stack) });
    return s;
  });
  const [detaching, setDetaching] = useState(false);
  const refresh = async () => {
    await qc.invalidateQueries({ queryKey: keys.stack(env, stack) });
    await qc.invalidateQueries({ queryKey: keys.stacks(env) });
  };
  const path = { env, stack };
  const run = useMutation({
    mutationFn: async (what: "sync" | "resume") => {
      const { data } = what === "sync" ? await syncStackGit({ path }) : await resumeStackGit({ path });
      await refresh();
      if (data.outcome === "failed") toast.error(outcomeText(data));
      else toast.success(outcomeText(data));
    },
  });
  const save = useMutation({
    meta: { inline: true },
    mutationFn: async (f: GitForm) => {
      const { data } = await setStackGit({ path, body: gitBody(f) });
      if (data.webhook_secret) setSecret(data.webhook_secret);
      setForm(null);
      await refresh();
    },
  });
  const acceptHostKey = useMutation({
    mutationFn: async () => {
      await resetStackGitHostKey({ path });
      const { data } = await syncStackGit({ path });
      await refresh();
      if (data.outcome === "failed") toast.error(outcomeText(data));
      else toast.success(outcomeText(data));
    },
  });
  const rotate = useMutation({
    mutationFn: async () => {
      const { data } = await rotateStackGitSecret({ path });
      setSecret(data.webhook_secret);
    },
  });
  if (git.error)
    return (
      <p role="alert" className="text-sm text-destructive">
        {errorMessage(git.error)}
      </p>
    );
  if (!git.data) return <p className="text-sm text-muted-foreground">Loading…</p>;
  const g = git.data.git;
  const editing = form ?? gitFormFrom(g);
  return (
    <div className="grid gap-6">
      {g.paused && (
        <div role="status" className="flex flex-wrap items-center gap-3 rounded-md border p-3 text-sm">
          <span>Auto-sync paused after rollback.</span>
          {canOperate && (
            <Button size="sm" variant="outline" disabled={run.isPending} onClick={() => run.mutate("resume")}>
              Resume
            </Button>
          )}
        </div>
      )}
      <section aria-label="Sync" className="grid gap-2">
        <dl className="grid grid-cols-[max-content_minmax(0,1fr)] gap-x-4 gap-y-1 text-sm">
          <dt className="text-muted-foreground">Fetched</dt>
          <dd className="font-ident">{g.last_fetched_commit ? short(g.last_fetched_commit) : "not yet"}</dd>
          <dt className="text-muted-foreground">Last sync</dt>
          <dd>{g.last_sync_at ? formatAgo(g.last_sync_at) : "never"}</dd>
          {g.ssh_host_key && (
            <>
              <dt className="text-muted-foreground">Host key</dt>
              <dd className="font-ident break-all">
                {`${g.ssh_host_key} (${PUBLISHED_HOST_KEYS.includes(sshHost(g.url)) ? "published key" : "trusted on first sync"})`}
              </dd>
            </>
          )}
        </dl>
        {g.last_sync_error && (
          <div className="flex flex-wrap items-center gap-3">
            <p role="alert" className="text-sm text-destructive">
              {g.last_sync_error}
            </p>
            {canEdit && hostKeyChanged(g.last_sync_error) && (
              <Button
                size="sm"
                variant="outline"
                disabled={acceptHostKey.isPending}
                onClick={() => acceptHostKey.mutate()}
              >
                Accept new key
              </Button>
            )}
          </div>
        )}
        {g.pending_commit && (
          <div className="flex flex-wrap items-center gap-3 text-sm">
            <span>
              Update available: <span className="font-ident">{short(g.pending_commit)}</span>
            </span>
            {canOperate && (
              <Button size="sm" onClick={() => onReview({ source: "draft" })}>
                Review deploy
              </Button>
            )}
          </div>
        )}
        {canOperate && (
          <div>
            <Button size="sm" variant="outline" disabled={run.isPending} onClick={() => run.mutate("sync")}>
              Sync now
            </Button>
          </div>
        )}
      </section>
      {g.sync === "webhook" && (
        <section aria-label="Webhook" className="grid gap-1">
          <h2 className="text-base font-semibold">Webhook</h2>
          <div className="flex flex-wrap items-center gap-2">
            <pre className="min-w-0 flex-1 overflow-x-auto rounded-md bg-muted p-2 font-mono text-xs">
              {absoluteUrl(git.data.webhook_url)}
            </pre>
            <Button size="sm" variant="outline" onClick={() => void copy(absoluteUrl(git.data.webhook_url))}>
              Copy
            </Button>
            {canEdit && (
              <Button size="sm" variant="outline" disabled={rotate.isPending} onClick={() => rotate.mutate()}>
                Rotate secret
              </Button>
            )}
          </div>
        </section>
      )}
      <section aria-label="Source" className="grid max-w-2xl gap-3">
        <h2 className="text-base font-semibold">Source</h2>
        <GitSettingsForm
          value={editing}
          onChange={setForm}
          hasToken={g.has_token}
          publicKey={g.ssh_public_key}
          disabled={!canEdit}
        />
        <FormError message={save.error ? errorMessage(save.error) : null} />
        {canEdit && (
          <div className="flex flex-wrap gap-2">
            <Button disabled={!form || !gitFormValid(editing) || save.isPending} onClick={() => save.mutate(editing)}>
              Save
            </Button>
            <Button variant="outline" onClick={() => setDetaching(true)}>
              Detach from git
            </Button>
          </div>
        )}
      </section>
      {secret && <WebhookSecretDialog url={git.data.webhook_url} secret={secret} onClose={() => setSecret(null)} />}
      <ConfirmDialog
        open={detaching}
        onOpenChange={setDetaching}
        title={`Detach ${stack} from git`}
        description="The stack keeps its Compose file, variables and revisions, and is edited in FleetOps from then on."
        confirmLabel="Detach"
        onConfirm={async () => {
          await detachStackGit({ path });
          await refresh();
        }}
      />
    </div>
  );
}
