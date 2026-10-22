import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { removeStackFile, setStackFile } from "@/api/gen/sdk.gen";
import type { ComposeIssue, StackFile } from "@/api/gen/types.gen";
import { keys, stackFilesQuery, stackGitQuery } from "@/api/queries";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { formatAgo } from "@/lib/format";
import { errorMessage } from "@/lib/errors";
import { FormError } from "@/pages/AuthLayout";
import { repoFileLink } from "./StackEditor";

/** The end of a "has no value" issue, which the Secrets tab fixes. */
const FIX_IN_TAB = "set it in the Secrets tab";

/** One Compose issue, with a way to the Secrets tab when that is where it is fixed. */
export function IssueLine({ issue, onSecrets }: { issue: ComposeIssue; onSecrets?: () => void }) {
  return (
    <>
      <span className="font-ident">{issue.path || "file"}</span>: {issue.message}
      {onSecrets && issue.message.endsWith(FIX_IN_TAB) && (
        <Button variant="link" size="sm" className="h-auto px-1 py-0" onClick={onSecrets}>
          Secrets tab
        </Button>
      )}
    </>
  );
}

const status = (f: StackFile) => (!f.used ? "Not used" : f.set ? "Set" : "Not set");

/** What a value is set to: a password field for a secret, never prefilled; the text of a config. */
function ValueDialog({ env, stack, file, onClose }: { env: string; stack: string; file: StackFile; onClose: () => void }) {
  const qc = useQueryClient();
  const secret = file.kind === "secret";
  const [value, setValue] = useState(secret ? "" : (file.content ?? ""));
  const save = useMutation({
    meta: { inline: true },
    mutationFn: async () => {
      await setStackFile({ path: { env, stack, kind: file.kind, name: file.name }, body: { value } });
      await qc.invalidateQueries({ queryKey: keys.stackFiles(env, stack) });
      await qc.invalidateQueries({ queryKey: keys.stack(env, stack) });
      await qc.invalidateQueries({ queryKey: keys.stacks(env) });
      onClose();
    },
  });
  const title = secret ? `${file.set ? "Replace" : "Set"} secret ${file.name}` : `Edit config ${file.name}`;
  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent className={secret ? undefined : "sm:max-w-3xl"}>
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
          <DialogDescription>
            {secret
              ? "Stored encrypted and never shown again. The stack's services get it on their next deploy."
              : "The stack's services get the new text on their next deploy."}
          </DialogDescription>
        </DialogHeader>
        <form
          className="grid gap-3"
          onSubmit={(e) => {
            e.preventDefault();
            save.mutate();
          }}
        >
          <div className="grid gap-1">
            <Label htmlFor="stack-file-value">{secret ? "Value" : "Content"}</Label>
            {secret ? (
              <Input
                id="stack-file-value"
                type="password"
                autoComplete="new-password"
                value={value}
                onChange={(e) => setValue(e.target.value)}
              />
            ) : (
              <Textarea
                id="stack-file-value"
                className="max-h-[50vh] font-mono text-xs"
                value={value}
                onChange={(e) => setValue(e.target.value)}
              />
            )}
          </div>
          <FormError message={save.error ? errorMessage(save.error) : null} />
          <DialogFooter>
            <Button type="button" variant="outline" onClick={onClose}>
              Cancel
            </Button>
            <Button type="submit" disabled={save.isPending || (secret && !value)}>
              Save
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

/** The secrets and configs a stack's Compose file declares, and the values stored for them. */
export function SecretsTab({ env, stack, isGit, canEdit }: { env: string; stack: string; isGit: boolean; canEdit: boolean }) {
  const qc = useQueryClient();
  const files = useQuery(stackFilesQuery(env, stack));
  const git = useQuery({ ...stackGitQuery(env, stack), enabled: isGit });
  const [editing, setEditing] = useState<StackFile | null>(null);
  const remove = useMutation({
    mutationFn: async (f: StackFile) => {
      await removeStackFile({ path: { env, stack, kind: f.kind, name: f.name } });
      await qc.invalidateQueries({ queryKey: keys.stackFiles(env, stack) });
      await qc.invalidateQueries({ queryKey: keys.stack(env, stack) });
      await qc.invalidateQueries({ queryKey: keys.stacks(env) });
    },
  });
  if (files.error) return <p role="alert" className="text-sm text-destructive">{errorMessage(files.error)}</p>;
  if (!files.data) return <p className="text-sm text-muted-foreground">Loading…</p>;
  if (files.data.length === 0)
    return (
      <p className="text-sm text-muted-foreground">
        The Compose file declares no <span className="font-ident">secrets</span> or{" "}
        <span className="font-ident">configs</span>.
      </p>
    );
  const source = (f: StackFile) => {
    switch (f.source) {
      case "variable":
        return <span className="font-ident">variable {f.variable}</span>;
      case "inline":
        return "inline";
      case "repo": {
        const g = git.data?.git;
        const href = g && f.path ? repoFileLink(g.url, g.branch, f.path) : null;
        return href ? (
          <a href={href} target="_blank" rel="noreferrer" className="font-ident underline">
            {f.path}
          </a>
        ) : (
          <span className="font-ident">{f.path}</span>
        );
      }
      default:
        return "Secrets tab";
    }
  };
  return (
    <div className="grid gap-3">
      <p className="text-sm text-muted-foreground">
        Values are stored encrypted and copied into each container before it starts. Secrets are never shown again
        once saved. Changes apply on the next deploy.
      </p>
      <table aria-label="Secrets" className="w-full text-sm">
        <thead className="text-left text-muted-foreground">
          <tr>
            <th className="py-1 pr-3 font-medium">Kind</th>
            <th className="py-1 pr-3 font-medium">Name</th>
            <th className="py-1 pr-3 font-medium">Source</th>
            <th className="py-1 pr-3 font-medium">Used by</th>
            <th className="py-1 pr-3 font-medium">Status</th>
            <th className="py-1 pr-3 font-medium">Updated</th>
            <th />
          </tr>
        </thead>
        <tbody>
          {files.data.map((f) => {
            const stored = f.source === "fleetops";
            return (
              <tr key={`${f.kind}/${f.source}/${f.name}`} className="border-t align-top">
                <td className="py-2 pr-3">{f.kind}</td>
                <td className="py-2 pr-3 font-ident">{f.name}</td>
                <td className="py-2 pr-3">{source(f)}</td>
                <td className="py-2 pr-3 font-ident">{f.used_by.join(", ")}</td>
                <td className={f.used && !f.set ? "py-2 pr-3 text-destructive" : "py-2 pr-3"}>{status(f)}</td>
                <td className="py-2 pr-3 text-muted-foreground">
                  {f.updated_at != null && `${formatAgo(f.updated_at)}${f.updated_by ? ` by ${f.updated_by}` : ""}`}
                </td>
                <td className="py-2 text-right whitespace-nowrap">
                  {canEdit && stored && (
                    <span className="inline-flex gap-2">
                      {f.used && (
                        <Button variant="outline" size="sm" onClick={() => setEditing(f)}>
                          {f.kind === "config" ? "Edit" : f.set ? "Replace" : "Set"}
                        </Button>
                      )}
                      {f.set && (
                        <Button variant="outline" size="sm" disabled={remove.isPending} onClick={() => remove.mutate(f)}>
                          Remove
                        </Button>
                      )}
                    </span>
                  )}
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
      {editing && <ValueDialog env={env} stack={stack} file={editing} onClose={() => setEditing(null)} />}
    </div>
  );
}
