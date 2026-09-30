import { useMutation } from "@tanstack/react-query";
import type { FormEvent, ReactNode } from "react";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { errorMessage } from "@/lib/errors";
import { FormError } from "@/pages/AuthLayout";

/** A dialog around a small form: submits `onSubmit`, closes when it resolves, shows its error when it fails. */
export function FormDialog(props: {
  title: string;
  description?: string;
  submitLabel: string;
  onSubmit: () => Promise<unknown>;
  onClose: () => void;
  children: ReactNode;
}) {
  const m = useMutation({ mutationFn: props.onSubmit, meta: { inline: true }, onSuccess: props.onClose });
  const submit = (e: FormEvent) => {
    e.preventDefault();
    m.mutate();
  };
  return (
    <Dialog open onOpenChange={(open) => !open && props.onClose()}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{props.title}</DialogTitle>
          {props.description && <DialogDescription>{props.description}</DialogDescription>}
        </DialogHeader>
        <form onSubmit={submit} className="grid gap-4">
          {props.children}
          <FormError message={m.error ? errorMessage(m.error) : null} />
          <DialogFooter>
            <Button type="submit" disabled={m.isPending}>
              {props.submitLabel}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
