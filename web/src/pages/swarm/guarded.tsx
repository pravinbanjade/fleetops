import { Check, X } from "lucide-react";
import { useMutation } from "@tanstack/react-query";
import { useState, type ReactNode } from "react";
import { ApiError } from "@/api/client";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { errorMessage } from "@/lib/errors";
import { FormError } from "@/pages/AuthLayout";

interface Ask {
  message: string;
  name: string;
  action: (confirm?: string) => Promise<unknown>;
  done: (ran: boolean) => void;
}

/** Asks for the object's name, then runs the action with it. */
function TypeToConfirm({ ask, close }: { ask: Ask; close: (ran: boolean) => void }) {
  const [typed, setTyped] = useState("");
  const m = useMutation({ mutationFn: () => ask.action(ask.name), meta: { inline: true } });
  return (
    <Dialog open onOpenChange={(open) => !open && close(false)}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Are you sure?</DialogTitle>
          <DialogDescription>{ask.message}</DialogDescription>
        </DialogHeader>
        <div className="grid gap-1.5">
          <Label htmlFor="type-to-confirm">
            Type <code className="font-mono">{ask.name}</code> to confirm
          </Label>
          <Input id="type-to-confirm" autoComplete="off" value={typed} onChange={(e) => setTyped(e.target.value)} />
        </div>
        <FormError message={m.error ? errorMessage(m.error) : null} />
        <DialogFooter>
          <Button variant="outline" onClick={() => close(false)}>
            <X aria-hidden />
            Cancel
          </Button>
          <Button variant="destructive" disabled={typed !== ask.name || m.isPending} onClick={() => m.mutate(undefined, { onSuccess: () => close(true) })}>
            <Check aria-hidden />
            Confirm
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

/**
 * Runs swarm changes that the server may guard (swarm spec §3): when it
 * answers `confirm_required`, asks for `name` and runs the action again with
 * it. `run` resolves true once the action ran, false when the user backed
 * out, and rejects with any other error.
 */
export function useGuarded(): { run: (action: (confirm?: string) => Promise<unknown>, name: string) => Promise<boolean>; dialog: ReactNode } {
  const [ask, setAsk] = useState<Ask | null>(null);
  const run = async (action: (confirm?: string) => Promise<unknown>, name: string) => {
    try {
      await action();
      return true;
    } catch (e) {
      if (!(e instanceof ApiError && e.code === "confirm_required")) throw e;
      return new Promise<boolean>((done) => setAsk({ message: e.message, name, action, done }));
    }
  };
  const dialog = ask && (
    <TypeToConfirm
      ask={ask}
      close={(ran) => {
        setAsk(null);
        ask.done(ran);
      }}
    />
  );
  return { run, dialog };
}
