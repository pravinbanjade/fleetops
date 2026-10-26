import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { getStackRevision } from "@/api/gen/sdk.gen";
import { stackRevisionsQuery } from "@/api/queries";
import { StateBadge } from "@/components/StateBadge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { formatAgo } from "@/lib/format";
import { errorMessage } from "@/lib/errors";

/** Every revision; any but the deployed one can be viewed and rolled back to. */
export function RevisionsTab({
  env,
  stack,
  deployed,
  canRollback,
  onRollback,
}: {
  env: string;
  stack: string;
  deployed: number | null | undefined;
  canRollback: boolean;
  onRollback: (revision: number) => void;
}) {
  const revs = useQuery(stackRevisionsQuery(env, stack));
  const [viewing, setViewing] = useState<number | null>(null);
  const shown = useQuery({
    queryKey: ["env", env, "stacks", stack, "revisions", viewing],
    queryFn: async () => (await getStackRevision({ path: { env, stack, n: viewing! } })).data,
    enabled: viewing != null,
  });
  if (revs.error)
    return (
      <p role="alert" className="text-sm text-destructive">
        {errorMessage(revs.error)}
      </p>
    );
  if (!revs.data) return <p className="text-sm text-muted-foreground">Loading…</p>;
  if (revs.data.length === 0) return <p className="text-sm text-muted-foreground">Not deployed yet.</p>;
  const commits = revs.data.some((r) => r.git_commit);
  return (
    <>
      <table aria-label="Revisions" className="w-full text-sm">
        <thead className="text-left text-muted-foreground">
          <tr>
            <th className="py-1 pr-3 font-medium">Revision</th>
            <th className="py-1 pr-3 font-medium">Kind</th>
            <th className="py-1 pr-3 font-medium">Status</th>
            {commits && <th className="py-1 pr-3 font-medium">Commit</th>}
            <th className="py-1 pr-3 font-medium">When</th>
            <th />
          </tr>
        </thead>
        <tbody>
          {revs.data.map((r) => (
            <tr key={r.number} className="border-t align-top">
              <td className="py-2 pr-3 font-ident">#{r.number}</td>
              <td className="py-2 pr-3">{r.kind === "rollback" ? `rollback of #${r.rollback_of}` : "deploy"}</td>
              <td className="py-2 pr-3">
                <StateBadge state={r.status} />
                {r.error && <div className="mt-1 text-xs text-destructive">{r.error}</div>}
              </td>
              {commits && <td className="py-2 pr-3 font-mono text-xs">{r.git_commit?.slice(0, 7) ?? ""}</td>}
              <td className="py-2 pr-3 text-muted-foreground">{formatAgo(r.created_at)}</td>
              <td className="py-2 text-right whitespace-nowrap">
                {r.number !== deployed && (
                  <span className="inline-flex gap-2">
                    <Button variant="outline" size="sm" onClick={() => setViewing(r.number)}>
                      View
                    </Button>
                    {canRollback && (
                      <Button variant="outline" size="sm" onClick={() => onRollback(r.number)}>
                        Roll back to this
                      </Button>
                    )}
                  </span>
                )}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      <Dialog open={viewing != null} onOpenChange={(o) => !o && setViewing(null)}>
        <DialogContent className="sm:max-w-3xl">
          <DialogHeader>
            <DialogTitle>Revision {viewing}</DialogTitle>
          </DialogHeader>
          <pre aria-label="Compose file" className="max-h-[60vh] overflow-auto rounded-md bg-[#0a0a0a] p-3 font-mono text-xs">
            {shown.data?.revision.compose ?? "Loading…"}
          </pre>
          {shown.data && shown.data.variables.length > 0 && (
            <p className="text-sm">
              Variables: <span className="font-ident">{shown.data.variables.join(", ")}</span>
            </p>
          )}
          {shown.data && shown.data.files.length > 0 && (
            <p className="text-sm">
              Files:{" "}
              <span className="font-ident">{shown.data.files.map((f) => `${f.kind} ${f.name}`).join(", ")}</span>
            </p>
          )}
        </DialogContent>
      </Dialog>
    </>
  );
}
