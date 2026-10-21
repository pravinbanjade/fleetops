import { useState } from "react";
import type { GitBody, GitRecord } from "@/api/gen/types.gen";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { NativeSelect } from "@/components/ui/native-select";

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
});

/** The client-side half of the server's URL rule; the server has the last word. */
export function gitUrlProblem(url: string): string | null {
  if (!url) return null;
  let u: URL;
  try {
    u = new URL(url);
  } catch {
    return "Only https git URLs are supported.";
  }
  if (u.protocol !== "https:") return "Only https git URLs are supported.";
  if (u.username || u.password) return "Put the access token in the token field, not in the URL.";
  return null;
}

export const gitFormValid = (f: GitForm) =>
  f.url !== "" && gitUrlProblem(f.url) === null && f.branch.trim() !== "" && (f.sync !== "poll" || Number(f.minutes) >= 1);

export const gitBody = (f: GitForm): GitBody => ({
  url: f.url.trim(),
  branch: f.branch.trim(),
  path: f.path.trim() || null,
  sync: f.sync,
  poll_seconds: f.sync === "poll" ? Math.round(Number(f.minutes) * 60) : null,
  auto_deploy: f.autoDeploy,
  token: f.token || null,
  clear_token: f.clearToken,
});

/** Where a stack's Compose file comes from in git, and how it syncs. */
export function GitSettingsForm({
  value,
  onChange,
  hasToken,
  disabled,
}: {
  value: GitForm;
  onChange: (f: GitForm) => void;
  /** A token is stored; it is never shown. */
  hasToken: boolean;
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
          placeholder="https://github.com/acme/ops.git"
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
