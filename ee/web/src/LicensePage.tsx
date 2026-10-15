import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { toast } from "sonner";
import { editionQuery } from "@/api/queries";
import { Page } from "@/components/Page";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import { formatTime } from "@/lib/format";
import { licenseQuery, removeLicense, uploadLicense, type LicenseInfo } from "./license";

export function LicensePage() {
  const qc = useQueryClient();
  const license = useQuery(licenseQuery);
  const [text, setText] = useState("");
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
  return (
    <Page title="License">
      {l && (
        <dl aria-label="License details" className="grid max-w-xl grid-cols-[10rem_1fr] gap-x-4 gap-y-2 text-sm">
          <dt className="text-muted-foreground">Status</dt>
          <dd>
            <Badge variant={l.status === "valid" ? "secondary" : "destructive"}>{l.status}</Badge>
            {l.reason && <span className="ml-2">{l.reason}</span>}
          </dd>
          <dt className="text-muted-foreground">Customer</dt>
          <dd>{l.customer ?? "—"}</dd>
          <dt className="text-muted-foreground">Expires</dt>
          <dd>{l.expires_at ? formatTime(l.expires_at) : "—"}</dd>
          <dt className="text-muted-foreground">Users</dt>
          <dd>
            {l.users_in_use} of {l.max_users ?? "unlimited"}
          </dd>
          <dt className="text-muted-foreground">Features</dt>
          <dd>{l.features.join(", ") || "none"}</dd>
        </dl>
      )}
      <form
        className="grid max-w-xl gap-2"
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
          className="font-mono text-xs"
          value={text}
          onChange={(e) => setText(e.target.value)}
        />
        <div className="flex gap-2">
          <Button type="submit" disabled={!text.trim() || upload.isPending}>
            Install license
          </Button>
          {l && l.status !== "none" && (
            <Button type="button" variant="outline" disabled={remove.isPending} onClick={() => remove.mutate()}>
              Remove license
            </Button>
          )}
        </div>
      </form>
    </Page>
  );
}
