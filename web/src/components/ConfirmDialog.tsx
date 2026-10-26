import { Check, TriangleAlert, X } from "lucide-react";
import { useMutation } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { errorMessage } from "@/lib/errors";
import { FormError } from "@/pages/AuthLayout";

interface ConfirmDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  description?: ReactNode;
  confirmLabel: string;
  /** Runs on confirm; the dialog closes when it resolves and shows the error when it rejects. */
  onConfirm: () => Promise<unknown>;
  destructive?: boolean;
  /** Options shown above the buttons, such as a "force" checkbox. */
  children?: ReactNode;
}

/** Asks before an action and runs it, keeping the dialog open with the error if it fails. */
export function ConfirmDialog(props: ConfirmDialogProps) {
  const m = useMutation({ mutationFn: props.onConfirm, meta: { inline: true } });
  const change = (open: boolean) => {
    if (!open) m.reset();
    props.onOpenChange(open);
  };
  return (
    <Dialog open={props.open} onOpenChange={change}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{props.title}</DialogTitle>
          {props.description && <DialogDescription>{props.description}</DialogDescription>}
        </DialogHeader>
        {props.children}
        <FormError message={m.error ? errorMessage(m.error) : null} />
        <DialogFooter>
          <Button variant="outline" onClick={() => change(false)}>
            <X aria-hidden />
            Cancel
          </Button>
          <Button
            variant={props.destructive === false ? "default" : "destructive"}
            disabled={m.isPending}
            onClick={() => m.mutate(undefined, { onSuccess: () => change(false) })}
          >
            {props.destructive === false ? <Check aria-hidden /> : <TriangleAlert aria-hidden />}
            {props.confirmLabel}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
