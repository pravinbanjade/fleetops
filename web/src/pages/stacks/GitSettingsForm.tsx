import { useState } from "react";
import { toast } from "sonner";
import type { GitBody, GitRecord } from "@/api/gen/types.gen";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { NativeSelect } from "@/components/ui/native-select";
import { Textarea } from "@/components/ui/textarea";

export type GitForm = {
  url: string;
  branch: string;
  path: string;
  sync: "manual" | "poll" | "webhook";
  minutes: string;
  autoDeploy: boolean;
  /** A new token; empty keeps the stored one. */
  token: string;
  clearToken: boolean;
  /** For ssh URLs: what saving does to the deploy key. */
  key: "keep" | "generate" | "paste" | "clear";
  /** A pasted private key, with `key: "paste"`. */
  sshKey: string;
};

export const emptyGitForm = (): GitForm => ({
  url: "",
  branch: "main",
  path: "compose.yaml",
  sync: "manual",
  minutes: "5",
  autoDeploy: false,
  token: "",
  clearToken: false,
  key: "generate",
  sshKey: "",
});

export const gitFormFrom = (g: GitRecord): GitForm => ({
  url: g.url,
  branch: g.branch,
  path: g.path,
  sync: g.sync as GitForm["sync"],
  minutes: String(Math.round(g.poll_seconds / 60)),
  autoDeploy: g.auto_deploy,
  token: "",
  clearToken: false,
  key: g.has_key ? "keep" : "generate",
  sshKey: "",
});

/** `ssh://…` or git's scp-like `[user@]host:path`, as the server reads them. */
export function isSshUrl(url: string): boolean {
  if (url.startsWith("ssh://")) return true;
  if (url.includes("://")) return false;
  const colon = url.indexOf(":");
  return colon >= 0 && !url.slice(0, colon).includes("/");
}

/** The client-side half of the server's URL rule; the server has the last word. */
export function gitUrlProblem(url: string): string | null {
  if (!url || isSshUrl(url)) return null;
  let u: URL;
  try {
    u = new URL(url);
  } catch {
    return "Only https and ssh git URLs are supported.";
  }
  if (u.protocol !== "https:") return "Only https and ssh git URLs are supported.";
  if (u.username || u.password) return "Put the access token in the token field, not in the URL.";
  return null;
}

export const gitFormValid = (f: GitForm) =>
  f.url !== "" &&
  gitUrlProblem(f.url) === null &&
  f.branch.trim() !== "" &&
  (f.sync !== "poll" || Number(f.minutes) >= 1) &&
  !(isSshUrl(f.url.trim()) && f.key === "paste" && !f.sshKey.trim());

/** The request body; only the credential of the URL's kind is sent. */
export const gitBody = (f: GitForm): GitBody => {
  const ssh = isSshUrl(f.url.trim());
  return {
    url: f.url.trim(),
    branch: f.branch.trim(),
    path: f.path.trim() || null,
    sync: f.sync,
    poll_seconds: f.sync === "poll" ? Math.round(Number(f.minutes) * 60) : null,
    auto_deploy: f.autoDeploy,
    token: ssh ? null : f.token || null,
    clear_token: !ssh && f.clearToken,
    generate_key: ssh && f.key === "generate",
    ssh_key: ssh && f.key === "paste" ? f.sshKey : null,
    clear_key: ssh && f.key === "clear",
  };
};

/** Where a stack's Compose file comes from in git, and how it syncs. */
export function GitSettingsForm({
  value,
  onChange,
  hasToken,
  publicKey = null,
  disabled,
}: {
  value: GitForm;
  onChange: (f: GitForm) => void;
  /** A token is stored; it is never shown. */
  hasToken: boolean;
  /** The stored SSH key's public line, when one is stored. */
  publicKey?: string | null;
  disabled?: boolean;
}) {
  const set = (p: Partial<GitForm>) => onChange({ ...value, ...p });
  const [replacing, setReplacing] = useState(false);
  const urlProblem = gitUrlProblem(value.url);
  const tokenSaved = hasToken && !value.clearToken && !replacing;
  return (
    <div className="grid gap-3">
      <div className="grid gap-1">
        <Label htmlFor="git-url">Repository URL</Label>
        <Input
          id="git-url"
          placeholder="https://github.com/acme/ops.git or git@github.com:acme/ops.git"
          value={value.url}
          disabled={disabled}
          aria-invalid={urlProblem !== null}
          onChange={(e) => set({ url: e.target.value })}
        />
        {urlProblem && <p className="text-sm text-destructive">{urlProblem}</p>}
      </div>
      <div className="grid gap-3 sm:grid-cols-2">
        <div className="grid gap-1">
          <Label htmlFor="git-branch">Branch</Label>
          <Input
            id="git-branch"
            className="font-ident"
            value={value.branch}
            disabled={disabled}
            onChange={(e) => set({ branch: e.target.value })}
          />
        </div>
        <div className="grid gap-1">
          <Label htmlFor="git-path">Compose file path</Label>
          <Input
            id="git-path"
            className="font-ident"
            value={value.path}
            disabled={disabled}
            onChange={(e) => set({ path: e.target.value })}
          />
        </div>
      </div>
      {isSshUrl(value.url.trim()) ? (
        <DeployKey value={value} set={set} publicKey={publicKey} disabled={disabled} />
      ) : (
        <div className="grid gap-1">
          <Label htmlFor="git-token">Access token</Label>
          {tokenSaved ? (
            <div className="flex flex-wrap items-center gap-2 text-sm">
              <span className="text-muted-foreground">Token saved</span>
              {!disabled && (
                <>
                  <Button type="button" variant="outline" size="sm" onClick={() => setReplacing(true)}>
                    Replace
                  </Button>
                  <Button type="button" variant="outline" size="sm" onClick={() => set({ clearToken: true, token: "" })}>
                    Remove
                  </Button>
                </>
              )}
            </div>
          ) : (
            <Input
              id="git-token"
              type="password"
              autoComplete="off"
              placeholder={value.clearToken ? "The token is removed when you save" : "Only for private repositories"}
              value={value.token}
              disabled={disabled}
              onChange={(e) => set({ token: e.target.value, clearToken: false })}
            />
          )}
        </div>
      )}
      <div className="grid gap-3 sm:grid-cols-2">
        <div className="grid gap-1">
          <Label htmlFor="git-sync">Sync</Label>
          <NativeSelect
            id="git-sync"
            value={value.sync}
            disabled={disabled}
            onChange={(e) => set({ sync: e.target.value as GitForm["sync"] })}
          >
            <option value="manual">By hand</option>
            <option value="poll">Polling</option>
            <option value="webhook">Webhook</option>
          </NativeSelect>
        </div>
        {value.sync === "poll" && (
          <div className="grid gap-1">
            <Label htmlFor="git-minutes">Every (minutes)</Label>
            <Input
              id="git-minutes"
              type="number"
              min={1}
              value={value.minutes}
              disabled={disabled}
              onChange={(e) => set({ minutes: e.target.value })}
            />
          </div>
        )}
      </div>
      <label className="flex items-center gap-2 text-sm">
        <Checkbox
          checked={value.autoDeploy}
          disabled={disabled}
          onCheckedChange={(c) => set({ autoDeploy: c === true })}
        />
        Deploy new commits automatically
      </label>
    </div>
  );
}

/** An ssh URL's deploy key: the stored one's public key, or a new one. */
function DeployKey({
  value,
  set,
  publicKey,
  disabled,
}: {
  value: GitForm;
  set: (p: Partial<GitForm>) => void;
  publicKey: string | null;
  disabled?: boolean;
}) {
  const hint = <p className="text-sm text-muted-foreground">Add the public key to the repository as a read-only deploy key.</p>;
  if (publicKey && value.key === "keep")
    return (
      <div className="grid gap-1">
        <span className="text-sm font-medium">Deploy key</span>
        <div className="flex flex-wrap items-center gap-2">
          <pre aria-label="Public key" className="min-w-0 flex-1 overflow-x-auto rounded-md bg-muted p-2 font-mono text-xs">
            {publicKey}
          </pre>
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={() => void navigator.clipboard.writeText(publicKey).then(() => toast.success("Copied"))}
          >
            Copy public key
          </Button>
          {!disabled && (
            <>
              <Button type="button" variant="outline" size="sm" onClick={() => set({ key: "generate" })}>
                Replace key
              </Button>
              <Button type="button" variant="outline" size="sm" onClick={() => set({ key: "clear" })}>
                Remove key
              </Button>
            </>
          )}
        </div>
        {hint}
      </div>
    );
  return (
    <div className="grid gap-1">
      <Label htmlFor="git-key">Deploy key</Label>
      {value.key === "clear" ? (
        <p className="text-sm text-muted-foreground">The key is removed when you save.</p>
      ) : (
        <>
          <NativeSelect
            id="git-key"
            value={value.key === "paste" ? "paste" : "generate"}
            disabled={disabled}
            onChange={(e) => set({ key: e.target.value as GitForm["key"] })}
          >
            <option value="generate">Generate a new key</option>
            <option value="paste">Paste a private key</option>
          </NativeSelect>
          {value.key === "paste" && (
            <Textarea
              aria-label="Private key"
              className="font-mono text-xs"
              rows={5}
              autoComplete="off"
              spellCheck={false}
              placeholder="-----BEGIN OPENSSH PRIVATE KEY----- (ed25519 or ECDSA, without a passphrase)"
              value={value.sshKey}
              disabled={disabled}
              onChange={(e) => set({ sshKey: e.target.value })}
            />
          )}
          {hint}
        </>
      )}
    </div>
  );
}
