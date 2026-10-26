import { Plus, X } from "lucide-react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { useState } from "react";
import { createStack, setStackGit } from "@/api/gen/sdk.gen";
import { keys } from "@/api/queries";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { errorMessage } from "@/lib/errors";
import { FormError } from "@/pages/AuthLayout";
import { GitSettingsForm, emptyGitForm, gitBody, gitFormValid } from "./GitSettingsForm";
import { secretKey } from "./GitTab";

/** The same rule the server applies to stack (Compose project) names. */
export const STACK_NAME = /^[a-z0-9][a-z0-9_-]{0,62}$/;

export const STACK_TEMPLATE = "services:\n  web:\n    image: nginx:1.27\n";

export function NewStackDialog({ env, onClose }: { env: string; onClose: () => void }) {
  const qc = useQueryClient();
  const navigate = useNavigate();
  const [name, setName] = useState("");
  const [fromGit, setFromGit] = useState(false);
  const [git, setGit] = useState(emptyGitForm);
  const nameValid = STACK_NAME.test(name);
  const valid = nameValid && (!fromGit || gitFormValid(git));
  const create = useMutation({
    mutationFn: async () => {
      await createStack({ path: { env }, body: { name, compose: fromGit ? "" : STACK_TEMPLATE, variables: [] } });
      if (fromGit) {
        const { data } = await setStackGit({ path: { env, stack: name }, body: gitBody(git) });
        // The stack page shows it once.
        if (data.webhook_secret) qc.setQueryData(secretKey(env, name), data.webhook_secret);
      }
      await qc.invalidateQueries({ queryKey: keys.stacks(env) });
      await navigate({ to: "/envs/$env/stacks/$stack", params: { env, stack: name } });
    },
    meta: { inline: true },
  });
  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>New stack</DialogTitle>
          <DialogDescription>
            The name is the Compose project name: containers are named after it, as <span className="font-ident">name-service-1</span>.
          </DialogDescription>
        </DialogHeader>
        <form
          className="grid gap-2"
          onSubmit={(e) => {
            e.preventDefault();
            if (valid) create.mutate();
          }}
        >
          <Label htmlFor="stack-name">Name</Label>
          <Input
            id="stack-name"
            autoFocus
            value={name}
            onChange={(e) => setName(e.target.value)}
            aria-invalid={name !== "" && !nameValid}
            className="font-ident"
          />
          {name !== "" && !nameValid && (
            <p className="text-sm text-destructive">
              Use lowercase letters, digits, &quot;_&quot; and &quot;-&quot;, starting with a letter or digit.
            </p>
          )}
          <fieldset className="mt-2 grid gap-2">
            <legend className="mb-1 text-sm font-medium">Compose file</legend>
            <label className="flex items-center gap-2 text-sm">
              <input type="radio" name="stack-source" checked={!fromGit} onChange={() => setFromGit(false)} />
              Write in FleetOps
            </label>
            <label className="flex items-center gap-2 text-sm">
              <input type="radio" name="stack-source" checked={fromGit} onChange={() => setFromGit(true)} />
              From a git repository
            </label>
          </fieldset>
          {fromGit && <GitSettingsForm value={git} onChange={setGit} hasToken={false} />}
          <FormError message={create.error ? errorMessage(create.error) : null} />
          <DialogFooter>
            <Button type="button" variant="outline" onClick={onClose}>
              <X aria-hidden />
              Cancel
            </Button>
            <Button type="submit" disabled={!valid || create.isPending}>
              <Plus aria-hidden />
              Create
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
