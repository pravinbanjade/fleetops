import { Tag } from "lucide-react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useState, type FormEvent } from "react";
import { toast } from "sonner";
import { keys } from "@/api/queries";
import { deleteImage, pruneImages, tagImage } from "@/api/gen/sdk.gen";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { bulkMessage, runBulk } from "@/lib/bulk";
import { errorMessage } from "@/lib/errors";
import { formatBytes, shortId } from "@/lib/format";
import { parseRef } from "@/lib/images";
import { FormError } from "@/pages/AuthLayout";

/** An image's first tag, or its short id when it has none. */
export function imageName(tags: string[] | null | undefined, id: string) {
  return tags?.[0] ?? shortId(id);
}

export function RemoveImagesDialog({
  env,
  images,
  onDone,
}: {
  env: string;
  images: { id: string; name: string }[];
  onDone: (removed: boolean) => void;
}) {
  const qc = useQueryClient();
  const [force, setForce] = useState(false);
  const one = images.length === 1;
  return (
    <ConfirmDialog
      open
      onOpenChange={(open) => !open && onDone(false)}
      title={one ? `Remove ${images[0].name}?` : `Remove ${images.length} images?`}
      description={one ? undefined : images.map((i) => i.name).join(", ")}
      confirmLabel="Remove"
      onConfirm={async () => {
        const r = await runBulk(images, (i) => i.name, (i) => deleteImage({ path: { env, id: i.id }, query: { force } }));
        await qc.invalidateQueries({ queryKey: keys.images(env) });
        if (r.ok === 0) throw one ? r.failed[0].error : new Error(bulkMessage("Removed", "image", r));
        const msg = bulkMessage("Removed", "image", r);
        if (r.failed.length) toast.error(msg);
        else toast.success(msg);
        onDone(true);
      }}
    >
      <div className="flex items-center gap-2">
        <Checkbox id="img-force" checked={force} onCheckedChange={(v) => setForce(!!v)} />
        <Label htmlFor="img-force">Force (remove every tag, even if a stopped container uses it)</Label>
      </div>
    </ConfirmDialog>
  );
}

export function PruneImagesDialog({ env, onDone }: { env: string; onDone: () => void }) {
  const qc = useQueryClient();
  const [all, setAll] = useState(false);
  return (
    <ConfirmDialog
      open
      onOpenChange={(open) => !open && onDone()}
      title="Prune images?"
      description="Removes dangling images: untagged layers no container uses."
      confirmLabel="Prune"
      onConfirm={async () => {
        const { data } = await pruneImages({ path: { env }, query: { all } });
        await qc.invalidateQueries({ queryKey: keys.images(env) });
        toast.success(`Removed ${data.deleted.length} images, reclaimed ${formatBytes(data.space_reclaimed)}`);
        onDone();
      }}
    >
      <div className="flex items-center gap-2">
        <Checkbox id="prune-all" checked={all} onCheckedChange={(v) => setAll(!!v)} />
        <Label htmlFor="prune-all">Also remove every image no container uses</Label>
      </div>
    </ConfirmDialog>
  );
}

/** Adds a `repo:tag` to an image. */
export function TagDialog({ env, id, onClose }: { env: string; id: string; onClose: () => void }) {
  const qc = useQueryClient();
  const [ref, setRef] = useState("");
  const m = useMutation({
    meta: { inline: true },
    mutationFn: () => tagImage({ path: { env, id }, body: parseRef(ref) }),
    onSuccess: async () => {
      await qc.invalidateQueries({ queryKey: keys.images(env) });
      toast.success(`Tagged ${ref}`);
      onClose();
    },
  });
  const submit = (e: FormEvent) => {
    e.preventDefault();
    m.mutate();
  };
  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Tag image</DialogTitle>
        </DialogHeader>
        <form onSubmit={submit} className="grid gap-4">
          <div className="grid gap-2">
            <Label htmlFor="tag-ref">New tag</Label>
            <Input id="tag-ref" required placeholder="app:1.2" value={ref} onChange={(e) => setRef(e.target.value)} />
          </div>
          <FormError message={m.error ? errorMessage(m.error) : null} />
          <DialogFooter>
            <Button type="submit" disabled={m.isPending}>
              <Tag aria-hidden />
              Tag
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
